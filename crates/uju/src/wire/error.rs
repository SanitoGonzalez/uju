use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum Error {
    #[error("buffer truncated")]
    Truncated,
    #[error("value exceeds its encodable range")]
    TooLarge,
    #[error("bool is neither 0 nor 1")]
    BadBool,
    #[error("no such enum variant")]
    BadEnum,
    #[error("timestamp is out of range")]
    BadTimestamp,
    #[error("interval is negative")]
    BadInterval,
    #[error("entity index is null")]
    BadEntity,
    #[error("string is not valid UTF-8")]
    BadUtf8,
    #[error("offset is not canonical")]
    BadOffset,
    #[error("absent optional slot is not zero-filled")]
    BadPadding,
    #[error("set or map keys are not sorted")]
    Unsorted,
    #[error("duplicate set element or map key")]
    Duplicate,
}

pub type Result<T> = core::result::Result<T, Error>;

pub fn need(bytes: &[u8], len: usize) -> Result<()> {
    if bytes.len() < len {
        Err(Error::Truncated)
    } else {
        Ok(())
    }
}
