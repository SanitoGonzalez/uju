use bytes::Bytes;
use compio::driver::BufferRef;
use compio::io::{AsyncReadExt, AsyncReadManaged, AsyncWrite, AsyncWriteExt};
use compio::net::TcpStream;
use compio::runtime::{spawn, JoinHandle};
use compio::BufResult;
use tokio::sync::mpsc;
use tracing::{debug, warn};

const INGRESS_CHANNEL_SIZE: usize = 64;

pub struct Connection {
    egress_tx: Option<mpsc::UnboundedSender<Bytes>>,
    ingress_rx: mpsc::Receiver<BufferRef>,

    recv_handle: Option<JoinHandle<()>>,
    send_handle: Option<JoinHandle<()>>,
}

struct Header {
    len: u16,
    id: u16,
}

impl Connection {
    pub fn open(stream: TcpStream) -> Self {
        let (reader, writer) = stream.into_split();
        let (egress_tx, egress_rx) = mpsc::unbounded_channel();
        let (ingress_tx, ingress_rx) = mpsc::channel(INGRESS_CHANNEL_SIZE);

        let recv_handle = spawn(Self::init_recv(reader, ingress_tx));
        let send_handle = spawn(Self::init_send(writer, egress_rx));

        Self {
            egress_tx: Some(egress_tx),
            ingress_rx,
            recv_handle: Some(recv_handle),
            send_handle: Some(send_handle),
        }
    }

    pub fn send(&self, buf: Bytes) {
        if let Some(tx) = &self.egress_tx {
            let _ = tx.send(buf);
        }
    }

    /// Stop accepting sends, flush everything already queued, then return.
    pub async fn close(&mut self) -> std::io::Result<()> {
        self.egress_tx = None; // closing the channel lets the send loop drain and exit
        self.recv_handle.take(); // dropping the handle cancels the recv task

        match self.send_handle.take() {
            Some(h) => h.await.unwrap_or(Ok(())),
            None => Ok(()),
        }
    }

    async fn init_recv(mut reader: TcpStream, ingress_tx: mpsc::Sender<BufferRef>) {
        async fn read(reader: &mut TcpStream) -> std::io::Result<Option<BufferRef>> {
            let header_buf = [0u8; Header::SIZE];
            let header = match reader.read_exact(header_buf).await {
                BufResult(Ok(()), header_buf) => Header::deserialize(&header_buf),
                BufResult(Err(e), _) => return Err(e.into()),
            };

            reader
                .read_managed(header.len as usize)
                .await
                .map_err(|e| e.into())
        }

        let peer = reader.peer_addr().ok();
        let span = tracing::info_span!("tcp recv", peer = ?peer);
        let _enter = span.enter();

        loop {
            let msg = match read(&mut reader).await {
                Ok(Some(msg)) => msg,
                Ok(None) => break,
                Err(e) => {
                    warn!(error = %e);
                    break;
                }
            };

            if let Err(_) = ingress_tx.send(msg).await {
                break;
            }
        }

        debug!("end");
    }

    async fn init_send(mut writer: TcpStream, mut egress_rx: mpsc::UnboundedReceiver<Bytes>) {
        let peer = writer.peer_addr().ok();
        let span = tracing::info_span!("tcp send", peer = ?peer);
        let _enter = span.enter();

        let mut bufs = Vec::with_capacity(8);
        while egress_rx.recv_many(&mut bufs, 8).await > 0 {
            for buf in bufs.drain(..) {
                if let BufResult(Err(e), _) = writer.write_all(buf).await {
                    warn!(error = %e);
                    break;
                }
            }
        }

        if let Err(e) = writer.flush().await {
            warn!(error = %e);
        }

        debug!("end");
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // Best-effort flush: detach the send task so queued frames still drain
        if let Some(h) = self.send_handle.take() {
            h.detach();
        }
        // recv_handle drops here, cancelling the recv task.
    }
}

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
