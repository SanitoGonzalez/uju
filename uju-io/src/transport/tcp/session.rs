use std::mem::take;

use bytes::Bytes;
use compio::BufResult;
use compio::io::{AsyncReadMulti, AsyncWrite, AsyncWriteExt};
use compio::net::TcpStream;
use futures_util::{FutureExt, StreamExt, select};
use tracing::warn;

use crate::stop::{StopSource, StopToken};

type EgressTx = crossfire::MTx<crossfire::mpsc::List<Bytes>>;
type EgressRx = crossfire::AsyncRx<crossfire::mpsc::List<Bytes>>;

pub struct Session {
    egress_tx: EgressTx,
    close: StopSource,
}

struct Header {
    len: u16,
    id: u16,
}

impl Session {
    pub fn open(stream: TcpStream, token: &StopToken) -> Self {
        let (reader, writer) = stream.into_split();
        let (egress_tx, egress_rx) = crossfire::mpsc::unbounded_async();
        let (close, token) = token.child();

        compio::runtime::spawn(Self::send_loop(writer, egress_rx, token.clone())).detach();
        compio::runtime::spawn(Self::recv_loop(reader, token)).detach();

        Self { egress_tx, close }
    }

    pub fn close(&mut self) {
        self.close.request();
    }

    pub fn send(&self, buf: Bytes) {
        _ = self.egress_tx.send(buf);
    }

    async fn send_loop(mut writer: TcpStream, egress_rx: EgressRx, token: StopToken) {
        const BATCH: usize = 64; // todo: accept env variable (note: IOV_MAX=1024 is the max)

        fn fill_batch(batch: &mut Vec<Bytes>, egress_rx: &EgressRx) {
            while batch.len() < BATCH {
                let Ok(buf) = egress_rx.try_recv() else {
                    break;
                };
                batch.push(buf);
            }
        }

        async fn flush_batch(
            writer: &mut TcpStream,
            batch: &mut Vec<Bytes>,
        ) -> std::io::Result<()> {
            let BufResult(result, mut buf) = writer.write_vectored_all(take(batch)).await;
            buf.clear();
            *batch = buf;
            result
        }

        let peer = writer.peer_addr().ok();
        let span = tracing::info_span!("tcp send", peer = ?peer);
        let _enter = span.enter();

        let mut batch = Vec::new();

        loop {
            let head = select! {
                result = egress_rx.recv().fuse() => match result {
                    Ok(buf) => buf,
                    Err(_) => break,
                },
                _ = token.wait().fuse() => break,
            };

            batch.push(head);
            fill_batch(&mut batch, &egress_rx);

            if let Err(e) = flush_batch(&mut writer, &mut batch).await {
                warn!(error = %e);
                break;
            }
        }

        // drain all
        loop {
            fill_batch(&mut batch, &egress_rx);
            if batch.is_empty() {
                break;
            }

            if let Err(e) = flush_batch(&mut writer, &mut batch).await {
                warn!(error = %e);
                break;
            }
        }

        if let Err(e) = writer.flush().await {
            warn!(error = %e);
        }

        _ = writer.close().await;
    }

    async fn recv_loop(mut reader: TcpStream, token: StopToken) {
        fn drain_frames(mut buf: &[u8]) -> usize {
            let total = buf.len();

            while let Some(header) = buf.first_chunk::<{ Header::SIZE }>() {
                let header = Header::deserialize(header);
                let end = Header::SIZE + header.len as usize;
                let Some(_frame) = buf.get(Header::SIZE..end) else {
                    break;
                };

                // todo: handle received frame

                buf = &buf[end..];
            }

            total - buf.len()
        }

        let peer = reader.peer_addr().ok();
        let span = tracing::info_span!("tcp recv", peer = ?peer);
        let _enter = span.enter();

        let mut chunks = reader.read_multi(0).fuse();
        let mut carry = Vec::new();

        loop {
            let chunk = select! {
                next = chunks.next() => match next {
                    Some(Ok(chunk)) => chunk,
                    Some(Err(e)) => {
                        warn!(error = %e);
                        break;
                    }
                    None => break,
                },
                _ = token.wait().fuse() => break,
            };

            if carry.is_empty() {
                let consumed = drain_frames(&chunk);
                carry.extend_from_slice(&chunk[consumed..]);
            } else {
                carry.extend_from_slice(&chunk);
                let consumed = drain_frames(&carry);
                carry.drain(..consumed);
            }
        }

        // do not `reader.close().await` here, since `writer` might be awaiting `reader` to be dropped.
    }
}

// todo: generalize to codec
impl Header {
    const SIZE: usize = 4;

    fn serialize(&self, buf: &mut [u8; Header::SIZE]) {
        buf[0] = (self.len >> 8) as u8;
        buf[1] = self.len as u8;
        buf[2] = (self.id >> 8) as u8;
        buf[3] = self.id as u8;
    }

    fn deserialize(buf: &[u8; Header::SIZE]) -> Self {
        let len = ((buf[0] as u16) << 8) | (buf[1] as u16);
        let id = (buf[2] as u16) << 8 | (buf[3] as u16);
        Self { len, id }
    }
}
