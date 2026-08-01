pub mod rudp;
pub mod tcp;

use bytes::Bytes;

#[derive(Debug, Clone, Copy)]
pub enum Protocol {
    Rudp,
    Tcp,
}

pub enum Connection {
    Rudp(rudp::Connection),
    Tcp(tcp::Connection),
}

pub enum Mode {
    ReliableOrdered,
    ReliableUnordered,
    Unreliable,
}

impl Connection {
    fn send(&mut self, buf: Bytes, _mode: Mode) {
        match self {
            Self::Rudp(_c) => todo!(),
            Self::Tcp(c) => c.send(buf),
        }
    }
}
