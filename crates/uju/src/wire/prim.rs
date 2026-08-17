use core::cmp::Ordering;

use crate::wire::error::{Error, Result, need};
use crate::wire::read::*;
use crate::wire::traits::{Canonical, View, Wire};
use crate::wire::write::Writer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Timestamp(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Interval(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Entity {
    pub index: u32,
    pub generation: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct UEntity {
    pub node: u16,
    pub shard: u16,
    pub index: u32,
    pub generation: u32,
}

macro_rules! scalar {
    ($ty:ty, $n:literal, $read:ident, $push:ident) => {
        impl Wire for $ty {
            const FIXED_SIZE: Option<usize> = Some($n);

            fn encoded_size(&self) -> usize {
                $n
            }

            fn encode(&self, w: &mut Writer) {
                w.$push(*self);
            }
        }

        impl<'a> View<'a> for $ty {
            type Owned = Self;

            const FIXED_SIZE: Option<usize> = Some($n);

            fn read(bytes: &'a [u8]) -> Self {
                $read(bytes, 0)
            }

            fn owned(self) -> Self {
                self
            }

            fn validate(bytes: &'a [u8]) -> Result<usize> {
                need(bytes, $n)?;
                Ok($n)
            }
        }
    };
}

macro_rules! ord_scalar {
    ($ty:ty, $n:literal, $read:ident, $push:ident) => {
        scalar!($ty, $n, $read, $push);

        impl Canonical for $ty {
            fn canonical_cmp(&self, other: &Self) -> Ordering {
                self.cmp(other)
            }
        }
    };
}

ord_scalar!(u8, 1, read_u8, push_u8);
ord_scalar!(u16, 2, read_u16, push_u16);
ord_scalar!(u32, 4, read_u32, push_u32);
ord_scalar!(u64, 8, read_u64, push_u64);
ord_scalar!(i8, 1, read_i8, push_i8);
ord_scalar!(i16, 2, read_i16, push_i16);
ord_scalar!(i32, 4, read_i32, push_i32);
ord_scalar!(i64, 8, read_i64, push_i64);

scalar!(f32, 4, read_f32, push_f32);
scalar!(f64, 8, read_f64, push_f64);

impl Canonical for f32 {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        self.total_cmp(other)
    }
}

impl Canonical for f64 {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        self.total_cmp(other)
    }
}

impl Wire for bool {
    const FIXED_SIZE: Option<usize> = Some(1);

    fn encoded_size(&self) -> usize {
        1
    }

    fn encode(&self, w: &mut Writer) {
        w.push_bool(*self);
    }
}

impl<'a> View<'a> for bool {
    type Owned = Self;

    const FIXED_SIZE: Option<usize> = Some(1);

    fn owned(self) -> Self {
        self
    }

    fn read(bytes: &'a [u8]) -> Self {
        read_bool(bytes, 0)
    }

    fn validate(bytes: &'a [u8]) -> Result<usize> {
        need(bytes, 1)?;
        if bytes[0] > 1 {
            return Err(Error::BadBool);
        }
        Ok(1)
    }
}

impl Canonical for bool {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        self.cmp(other)
    }
}

macro_rules! newtype_i64 {
    ($ty:ty) => {
        impl Wire for $ty {
            const FIXED_SIZE: Option<usize> = Some(8);

            fn encoded_size(&self) -> usize {
                8
            }

            fn encode(&self, w: &mut Writer) {
                w.push_i64(self.0);
            }
        }

        impl<'a> View<'a> for $ty {
            type Owned = Self;

            const FIXED_SIZE: Option<usize> = Some(8);

            fn read(bytes: &'a [u8]) -> Self {
                Self(read_i64(bytes, 0))
            }

            fn owned(self) -> Self {
                self
            }

            fn validate(bytes: &'a [u8]) -> Result<usize> {
                need(bytes, 8)?;
                Ok(8)
            }
        }

        impl Canonical for $ty {
            fn canonical_cmp(&self, other: &Self) -> Ordering {
                self.cmp(other)
            }
        }
    };
}

newtype_i64!(Timestamp);
newtype_i64!(Interval);

impl Wire for Entity {
    const FIXED_SIZE: Option<usize> = Some(8);

    fn encoded_size(&self) -> usize {
        8
    }

    fn encode(&self, w: &mut Writer) {
        w.push_u32(self.index);
        w.push_u32(self.generation);
    }
}

impl<'a> View<'a> for Entity {
    type Owned = Self;

    const FIXED_SIZE: Option<usize> = Some(8);

    fn owned(self) -> Self {
        self
    }

    fn read(bytes: &'a [u8]) -> Self {
        Self {
            index: read_u32(bytes, 0),
            generation: read_u32(bytes, 4),
        }
    }

    fn validate(bytes: &'a [u8]) -> Result<usize> {
        need(bytes, 8)?;
        Ok(8)
    }
}

impl Canonical for Entity {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        self.cmp(other)
    }
}

impl Wire for UEntity {
    const FIXED_SIZE: Option<usize> = Some(12);

    fn encoded_size(&self) -> usize {
        12
    }

    fn encode(&self, w: &mut Writer) {
        w.push_u16(self.node);
        w.push_u16(self.shard);
        w.push_u32(self.index);
        w.push_u32(self.generation);
    }
}

impl<'a> View<'a> for UEntity {
    type Owned = Self;

    const FIXED_SIZE: Option<usize> = Some(12);

    fn owned(self) -> Self {
        self
    }

    fn read(bytes: &'a [u8]) -> Self {
        Self {
            node: read_u16(bytes, 0),
            shard: read_u16(bytes, 2),
            index: read_u32(bytes, 4),
            generation: read_u32(bytes, 8),
        }
    }

    fn validate(bytes: &'a [u8]) -> Result<usize> {
        need(bytes, 12)?;
        Ok(12)
    }
}

impl Canonical for UEntity {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        self.cmp(other)
    }
}

impl Wire for String {
    const FIXED_SIZE: Option<usize> = None;

    fn encoded_size(&self) -> usize {
        2 + self.len()
    }

    fn encode(&self, w: &mut Writer) {
        let len = w.short(self.len());
        w.push_u16(len);
        w.push_bytes(self.as_bytes());
    }
}

impl<'a> View<'a> for &'a str {
    type Owned = String;

    const FIXED_SIZE: Option<usize> = None;

    fn owned(self) -> String {
        self.to_string()
    }

    fn read(bytes: &'a [u8]) -> Self {
        let len = read_u16(bytes, 0) as usize;
        core::str::from_utf8(&bytes[2..2 + len]).unwrap_or_default()
    }

    fn validate(bytes: &'a [u8]) -> Result<usize> {
        need(bytes, 2)?;
        let len = read_u16(bytes, 0) as usize;
        need(bytes, 2 + len)?;
        core::str::from_utf8(&bytes[2..2 + len]).map_err(|_| Error::BadUtf8)?;
        Ok(2 + len)
    }
}

impl Canonical for &str {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        self.as_bytes().cmp(other.as_bytes())
    }
}

impl Canonical for String {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        self.as_bytes().cmp(other.as_bytes())
    }
}

impl<'a> View<'a> for &'a [u8] {
    type Owned = Vec<u8>;

    const FIXED_SIZE: Option<usize> = None;

    fn owned(self) -> Vec<u8> {
        self.to_vec()
    }

    fn read(bytes: &'a [u8]) -> Self {
        let len = read_u16(bytes, 0) as usize;
        &bytes[2..2 + len]
    }

    fn validate(bytes: &'a [u8]) -> Result<usize> {
        need(bytes, 2)?;
        let len = read_u16(bytes, 0) as usize;
        need(bytes, 2 + len)?;
        Ok(2 + len)
    }
}

impl Canonical for &[u8] {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        (*self).cmp(*other)
    }
}

impl<T: Wire + Canonical> Wire for Vec<T> {
    const FIXED_SIZE: Option<usize> = None;

    fn encoded_size(&self) -> usize {
        match T::FIXED_SIZE {
            Some(n) => 2 + self.len() * n,
            None => 2 + self.len() * 2 + self.iter().map(Wire::encoded_size).sum::<usize>(),
        }
    }

    fn encode(&self, w: &mut Writer) {
        let start = w.pos();
        let count = w.short(self.len());
        w.push_u16(count);
        if T::FIXED_SIZE.is_some() {
            for element in self {
                element.encode(w);
            }
        } else {
            let table = w.pos();
            w.push_zeros(self.len() * 2);
            for (i, element) in self.iter().enumerate() {
                let offset = w.short(w.pos() - start);
                w.put_u16(table + i * 2, offset);
                element.encode(w);
            }
        }
    }
}

impl<T: Canonical> Canonical for Vec<T> {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        for (a, b) in self.iter().zip(other) {
            match a.canonical_cmp(b) {
                Ordering::Equal => {}
                ord => return ord,
            }
        }
        self.len().cmp(&other.len())
    }
}
