pub mod rudp;
pub mod tcp;

use bytes::Bytes;

#[derive(Debug, Clone, Copy)]
pub enum Protocol {
    Rudp,
    Tcp,
}

pub enum Session {
    Rudp(rudp::Session),
    Tcp(tcp::Session),
}

pub enum Semantic {
    ReliableOrdered,
    ReliableUnordered,
    UnreliableSequenced,
    Unreliable,
}

impl Session {
    fn send(&mut self, buf: Bytes, _semantic: Semantic) {
        match self {
            Self::Rudp(_c) => todo!(),
            Self::Tcp(c) => c.send(buf),
        }
    }

    #[inline]
    fn send_ro(&mut self, buf: Bytes) {
        self.send(buf, Semantic::ReliableOrdered);
    }

    #[inline]
    fn send_ru(&mut self, buf: Bytes) {
        self.send(buf, Semantic::ReliableUnordered);
    }

    #[inline]
    fn send_us(&mut self, buf: Bytes) {
        self.send(buf, Semantic::UnreliableSequenced);
    }

    #[inline]
    fn send_u(&mut self, buf: Bytes) {
        self.send(buf, Semantic::Unreliable);
    }

    fn close(&mut self) {
        match self {
            Self::Rudp(_c) => todo!(),
            Self::Tcp(c) => c.close(),
        }
    }
}
