use std::net::SocketAddr;

use compio::net::TcpListener;
use futures_util::{FutureExt, select};
use socket2::{Domain, Protocol, Socket, Type};
use tracing::{debug, error};

use crate::stop::StopToken;
use crate::transport::tcp::session::Session;

pub type Listener = TcpListener;

pub fn bind(addr: SocketAddr) -> std::io::Result<Listener> {
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    socket.bind(&addr.into())?;
    socket.listen(libc::SOMAXCONN)?;
    socket.set_nonblocking(true)?;
    socket.set_tcp_nodelay(true)?;

    let listener: std::net::TcpListener = socket.into();
    TcpListener::from_std(listener)
}

pub fn run(listener: Listener, token: StopToken) {
    compio::runtime::spawn(async move {
        loop {
            select! {
                result = listener.accept().fuse() => match result {
                    Ok((stream, addr)) => {
                        debug!("accepted from: {addr}");
                        let session = Session::open(stream);
                        // todo: register session
                    }
                    Err(e) => {
                        error!("failed to accept: {e}");
                    }
                },
                _ = token.wait().fuse() => break,
            }
        }
    })
    .detach();
}
