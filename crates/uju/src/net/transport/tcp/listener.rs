use std::net::SocketAddr;

use compio::net::TcpListener;
use futures_util::{FutureExt, select};
use socket2::{Domain, Protocol, Socket, Type};
use tracing::{debug, error};

use crate::net::error::Result;
use crate::net::transport::tcp::session::Session;
use crate::util::stop::StopToken;

/// Binds a listener into the `SO_REUSEPORT` group of `addr`. With `cpu`, sets `SO_INCOMING_CPU`
/// so the kernel prefers this listener for SYNs processed on that cpu, falling back to the hash.
pub fn bind(addr: SocketAddr, cpu: Option<usize>) -> Result<TcpListener> {
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_port(true)?;
    if let Some(cpu) = cpu {
        socket.set_cpu_affinity(cpu)?;
    }
    // socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.set_tcp_nodelay(true)?;
    // socket.set_keepalive(true)?;
    socket.bind(&addr.into())?;
    socket.listen(libc::SOMAXCONN)?;

    let listener: std::net::TcpListener = socket.into();
    let listener = TcpListener::from_std(listener)?;
    Ok(listener)
}

pub async fn accept(listener: TcpListener, token: StopToken) {
    loop {
        select! {
            result = listener.accept().fuse() => match result {
                Ok((stream, addr)) => {
                    debug!("accepted from: {addr}");
                    let _session = Session::open(stream, &token);
                    // todo: register session
                }
                Err(e) => {
                    error!("failed to accept: {e}");
                }
            },
            _ = token.wait().fuse() => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::stop::StopSource;

    #[compio::test]
    async fn test_listener_bind() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener1 = bind(addr, None).unwrap();
        let _listener2 = bind(listener1.local_addr().unwrap(), None).unwrap();
    }

    #[compio::test]
    async fn test_listener_bind_incoming_cpu() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = bind(addr, Some(0)).unwrap();
        assert_eq!(socket2::SockRef::from(&listener).cpu_affinity().unwrap(), 0);
    }

    #[compio::test]
    async fn test_listener_accept() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = bind(addr, None).unwrap();
        let addr = listener.local_addr().unwrap();

        let (stop, token) = StopSource::new();
        let handle = compio::runtime::spawn(async move {
            accept(listener, token).await;
        });

        let socket = compio::net::TcpSocket::new_v4().await.unwrap();
        socket.connect(addr).await.unwrap();

        stop.request();
        handle.await.unwrap();
    }
}
