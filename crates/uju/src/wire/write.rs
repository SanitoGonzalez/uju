use crate::wire::error::{Error, Result};

pub struct Writer<'a> {
    buf: &'a mut [u8],
    pos: usize,
    overflow: bool,
}

macro_rules! writer {
    ($push:ident, $put:ident, $ty:ty, $n:literal) => {
        #[inline]
        pub fn $push(&mut self, value: $ty) {
            let at = self.pos;
            self.pos += $n;
            self.buf[at..at + $n].copy_from_slice(&value.to_le_bytes());
        }

        #[inline]
        pub fn $put(&mut self, at: usize, value: $ty) {
            self.buf[at..at + $n].copy_from_slice(&value.to_le_bytes());
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

    pub fn finish(self) -> Result<usize> {
        if self.overflow {
            Err(Error::TooLarge)
        } else {
            Ok(self.pos)
        }
    }

    #[inline]
    pub fn short(&mut self, value: usize) -> u16 {
        match u16::try_from(value) {
            Ok(v) => v,
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

    writer!(push_u16, put_u16, u16, 2);
    writer!(push_u32, put_u32, u32, 4);
    writer!(push_u64, put_u64, u64, 8);
    writer!(push_i16, put_i16, i16, 2);
    writer!(push_i32, put_i32, i32, 4);
    writer!(push_i64, put_i64, i64, 8);
    writer!(push_f32, put_f32, f32, 4);
    writer!(push_f64, put_f64, f64, 8);
}
