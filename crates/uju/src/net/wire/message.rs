use crate::net::wire::error::{Result, need};
use crate::net::wire::write::Writer;

/// A zero-copy view of an encoded message.
pub trait MessageView<'a>: Copy + Sized {
    type Owned: Message;

    /// Reads the message at the start of `bytes`, which
    /// [`MessageView::validate`] must have accepted.
    fn read(bytes: &'a [u8]) -> Self;

    /// Checks the message and returns its encoded length.
    fn validate(bytes: &'a [u8]) -> Result<usize>;

    /// Copies the message out of the buffer.
    fn to_owned(self) -> Self::Owned;
}

/// An owned message, which knows its encoded size before it is written.
pub trait Message: Sized {
    type View<'a>: MessageView<'a, Owned = Self>;

    /// Alignment a receive buffer should have, at most 8.
    const ALIGN: usize;

    fn encoded_size(&self) -> usize;

    /// Writes the message at the start of the buffer.
    fn write(&self, w: &mut Writer);

    /// Encodes into `buf`, which must hold [`Message::encoded_size`] bytes.
    fn encode_into(&self, buf: &mut [u8]) -> Result<usize> {
        need(buf, self.encoded_size())?;
        let mut w = Writer::new(buf);
        self.write(&mut w);
        w.finish()
    }

    fn encode(&self) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; self.encoded_size()];
        let len = self.encode_into(&mut buf)?;
        buf.truncate(len);
        Ok(buf)
    }

    /// Checks an encoded message and returns its length; whatever follows it
    /// in a larger receive buffer is left alone.
    fn validate(bytes: &[u8]) -> Result<usize> {
        <Self::View<'_> as MessageView<'_>>::validate(bytes)
    }

    /// Checks an encoded message and returns a view borrowing from `bytes`.
    fn decode(bytes: &[u8]) -> Result<Self::View<'_>> {
        Self::validate(bytes)?;
        Ok(<Self::View<'_> as MessageView<'_>>::read(bytes))
    }
}
