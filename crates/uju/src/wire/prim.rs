use core::cmp::Ordering;

use chrono::{DateTime, Utc};

pub use crate::ecs::entity::{Entity, UniversalEntity};
use crate::ecs::entity::{EntityGeneration, EntityIndex};
use crate::wire::error::{Error, Result, need};
use crate::wire::read::*;
use crate::wire::traits::{Canonical, View, Wire};
use crate::wire::write::Writer;

pub type Timestamp = DateTime<Utc>;
pub type Interval = core::time::Duration;

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

impl Wire for Timestamp {
    const FIXED_SIZE: Option<usize> = Some(8);

    fn encoded_size(&self) -> usize {
        8
    }

    fn encode(&self, w: &mut Writer) {
        w.push_i64(self.timestamp_micros());
    }
}

impl<'a> View<'a> for Timestamp {
    type Owned = Self;

    const FIXED_SIZE: Option<usize> = Some(8);

    fn read(bytes: &'a [u8]) -> Self {
        DateTime::from_timestamp_micros(read_i64(bytes, 0)).unwrap_or_default()
    }

    fn owned(self) -> Self {
        self
    }

    fn validate(bytes: &'a [u8]) -> Result<usize> {
        need(bytes, 8)?;
        match DateTime::<Utc>::from_timestamp_micros(read_i64(bytes, 0)) {
            Some(_) => Ok(8),
            None => Err(Error::BadTimestamp),
        }
    }
}

impl Canonical for Timestamp {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        self.cmp(other)
    }
}

impl Wire for Interval {
    const FIXED_SIZE: Option<usize> = Some(8);

    fn encoded_size(&self) -> usize {
        8
    }

    fn encode(&self, w: &mut Writer) {
        let micros = w.long(self.as_micros());
        w.push_i64(micros);
    }
}

impl<'a> View<'a> for Interval {
    type Owned = Self;

    const FIXED_SIZE: Option<usize> = Some(8);

    fn read(bytes: &'a [u8]) -> Self {
        Self::from_micros(read_i64(bytes, 0).max(0) as u64)
    }

    fn owned(self) -> Self {
        self
    }

    fn validate(bytes: &'a [u8]) -> Result<usize> {
        need(bytes, 8)?;
        if read_i64(bytes, 0) < 0 {
            return Err(Error::BadInterval);
        }
        Ok(8)
    }
}

impl Canonical for Interval {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        self.cmp(other)
    }
}

impl Wire for Entity {
    const FIXED_SIZE: Option<usize> = Some(8);

    fn encoded_size(&self) -> usize {
        8
    }

    fn encode(&self, w: &mut Writer) {
        w.push_u32(self.index().to_bits());
        w.push_u32(self.generation().to_bits());
    }
}

impl<'a> View<'a> for Entity {
    type Owned = Self;

    const FIXED_SIZE: Option<usize> = Some(8);

    fn owned(self) -> Self {
        self
    }

    fn read(bytes: &'a [u8]) -> Self {
        Self::new(
            EntityIndex::from_bits(read_u32(bytes, 0)),
            EntityGeneration::from_bits(read_u32(bytes, 4)),
        )
    }

    fn validate(bytes: &'a [u8]) -> Result<usize> {
        need(bytes, 8)?;
        if read_u32(bytes, 0) == EntityIndex::NULL.to_bits() {
            return Err(Error::BadEntity);
        }
        Ok(8)
    }
}

impl Canonical for Entity {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        self.index()
            .cmp(&other.index())
            .then_with(|| self.generation().cmp(&other.generation()))
    }
}

impl Wire for UniversalEntity {
    const FIXED_SIZE: Option<usize> = Some(Self::BYTES);

    fn encoded_size(&self) -> usize {
        Self::BYTES
    }

    fn encode(&self, w: &mut Writer) {
        w.push_u16(self.node());
        w.push_u16(self.shard());
        self.entity().encode(w);
    }
}

impl<'a> View<'a> for UniversalEntity {
    type Owned = Self;

    const FIXED_SIZE: Option<usize> = Some(Self::BYTES);

    fn owned(self) -> Self {
        self
    }

    fn read(bytes: &'a [u8]) -> Self {
        Self::new(
            read_u16(bytes, 0),
            read_u16(bytes, 2),
            Entity::read(&bytes[4..]),
        )
    }

    fn validate(bytes: &'a [u8]) -> Result<usize> {
        need(bytes, Self::BYTES)?;
        Entity::validate(&bytes[4..])?;
        Ok(Self::BYTES)
    }
}

impl Canonical for UniversalEntity {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        self.node()
            .cmp(&other.node())
            .then_with(|| self.shard().cmp(&other.shard()))
            .then_with(|| self.entity().canonical_cmp(&other.entity()))
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

impl<T: Canonical> Canonical for Option<T> {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            (Some(a), Some(b)) => a.canonical_cmp(b),
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
