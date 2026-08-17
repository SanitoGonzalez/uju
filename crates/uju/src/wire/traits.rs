use core::cmp::Ordering;

use crate::wire::error::Result;
use crate::wire::write::Writer;

pub trait Wire {
    const FIXED_SIZE: Option<usize>;

    fn encoded_size(&self) -> usize;

    fn encode(&self, w: &mut Writer);
}

pub trait View<'a>: Copy + Canonical + Sized {
    type Owned;

    const FIXED_SIZE: Option<usize>;

    fn read(bytes: &'a [u8]) -> Self;

    fn validate(bytes: &'a [u8]) -> Result<usize>;

    fn owned(self) -> Self::Owned;
}

pub trait Canonical {
    fn canonical_cmp(&self, other: &Self) -> Ordering;
}

pub trait Component: Wire {}

pub trait Message: Wire {
    const MESSAGE_ID: u32;
}

pub trait Request: Message {
    type Response: Message;
}

pub fn wire_size<T: Wire>(value: &T) -> usize {
    value.encoded_size()
}

pub fn encode_into<T: Wire>(value: &T, buf: &mut [u8]) -> Result<usize> {
    crate::wire::error::need(buf, value.encoded_size())?;
    let mut w = Writer::new(buf);
    value.encode(&mut w);
    w.finish()
}

pub fn encode<T: Wire>(value: &T) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; value.encoded_size()];
    let len = encode_into(value, &mut buf)?;
    buf.truncate(len);
    Ok(buf)
}

pub fn validate<'a, T: View<'a>>(bytes: &'a [u8]) -> Result<usize> {
    T::validate(bytes)
}

pub fn view<'a, T: View<'a>>(bytes: &'a [u8]) -> T {
    T::read(bytes)
}
