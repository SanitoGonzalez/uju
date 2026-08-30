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
    #[error("no such union member")]
    BadTag,
    #[error("timestamp is out of range")]
    BadTimestamp,
    #[error("interval is negative")]
    BadInterval,
    #[error("entity index is null")]
    BadEntity,
    #[error("string is not valid UTF-8")]
    BadUtf8,
    #[error("absent optional field is not zero-filled")]
    BadPadding,
    #[error("set items or map keys are not sorted")]
    Unsorted,
    #[error("duplicate set item or map key")]
    Duplicate,
}

pub type Result<T> = core::result::Result<T, Error>;

/// Checks that `bytes` reaches `end`, an offset from the start of the message.
pub fn need(bytes: &[u8], end: usize) -> Result<()> {
    if bytes.len() < end {
        Err(Error::Truncated)
    } else {
        Ok(())
    }
}
