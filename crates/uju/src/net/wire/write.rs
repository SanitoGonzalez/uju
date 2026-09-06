use crate::net::wire::error::{Error, Result};

/// An append-only cursor over a message buffer. Padding is zero-filled, so a
/// value is written by aligning — or seeking to a known offset — and pushing.
pub struct Writer<'a> {
    buf: &'a mut [u8],
    pos: usize,
    overflow: bool,
}

macro_rules! writer {
    ($push:ident, $ty:ty, $n:literal) => {
        #[inline]
        pub fn $push(&mut self, value: $ty) {
            let at = self.pos;
            self.pos += $n;
            self.buf[at..self.pos].copy_from_slice(&value.to_le_bytes());
        }
    };
}

impl<'a> Writer<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self {
            buf,
            pos: 0,
            overflow: false,
        }
    }

    #[inline]
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// The bytes written, or [`Error::TooLarge`] if a count, length, or
    /// duration did not fit its encoding.
    pub fn finish(self) -> Result<usize> {
        if self.overflow {
            Err(Error::TooLarge)
        } else {
            Ok(self.pos)
        }
    }

    /// Zero-fills up to `at`, an offset at or after the current position.
    #[inline]
    pub fn pad_to(&mut self, at: usize) {
        debug_assert!(at >= self.pos, "the writer only moves forward");
        self.push_zeros(at - self.pos);
    }

    /// Zero-fills up to the next multiple of `align`.
    #[inline]
    pub fn align(&mut self, align: usize) {
        self.pad_to(self.pos.next_multiple_of(align));
    }

    /// Writes a `u16` head: an item count, a byte length, or a union tag.
    #[inline]
    pub fn push_head(&mut self, value: usize) {
        let head = self.short(value);
        self.push_u16(head);
    }

    #[inline]
    pub fn short(&mut self, value: usize) -> u16 {
        match u16::try_from(value) {
            Ok(value) => value,
            Err(_) => {
                self.overflow = true;
                0
            }
        }
    }

    #[inline]
    pub fn long(&mut self, value: u128) -> i64 {
        match i64::try_from(value) {
            Ok(value) => value,
            Err(_) => {
                self.overflow = true;
                0
            }
        }
    }

    #[inline]
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        let at = self.pos;
        self.pos += bytes.len();
        self.buf[at..self.pos].copy_from_slice(bytes);
    }

    #[inline]
    pub fn push_zeros(&mut self, len: usize) {
        let at = self.pos;
        self.pos += len;
        self.buf[at..self.pos].fill(0);
    }

    #[inline]
    pub fn push_u8(&mut self, value: u8) {
        self.buf[self.pos] = value;
        self.pos += 1;
    }

    #[inline]
    pub fn push_i8(&mut self, value: i8) {
        self.push_u8(value as u8);
    }

    #[inline]
    pub fn push_bool(&mut self, value: bool) {
        self.push_u8(value as u8);
    }

    writer!(push_u16, u16, 2);
    writer!(push_u32, u32, 4);
    writer!(push_u64, u64, 8);
    writer!(push_i16, i16, 2);
    writer!(push_i32, i32, 4);
    writer!(push_i64, i64, 8);
    writer!(push_f32, f32, 4);
    writer!(push_f64, f64, 8);
}
