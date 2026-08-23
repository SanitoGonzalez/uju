use core::cmp::Ordering;
use core::fmt;

use chrono::{DateTime, Utc};

pub use crate::ecs::entity::{Entity, UniversalEntity};
use crate::ecs::entity::{EntityGeneration, EntityIndex};
use crate::wire::error::{Error, Result, need};
use crate::wire::read::*;
use crate::wire::traits::{Canonical, Layout, View, Wire, fixed_size};
use crate::wire::write::Writer;

pub type Timestamp = DateTime<Utc>;
pub type Interval = core::time::Duration;

/// A 16-byte UUID, ordered and compared bytewise.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Uuid(pub [u8; 16]);

impl Uuid {
    pub const SIZE: usize = 16;

    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, byte) in self.0.iter().enumerate() {
            if matches!(index, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

macro_rules! scalar {
    ($ty:ty, $n:literal, $read:ident, $push:ident) => {
        impl Layout for $ty {
            const FIXED_SIZE: Option<usize> = Some($n);
            const ALIGN: usize = $n;
        }

        impl Wire for $ty {
            fn encoded_size(&self) -> usize {
                $n
            }

            fn encode(&self, w: &mut Writer) {
                w.$push(*self);
            }
        }

        impl<'a> View<'a> for $ty {
            type Owned = Self;

            fn read(bytes: &'a [u8], at: usize, _head: u16) -> Self {
                $read(bytes, at)
            }

            fn end(_bytes: &'a [u8], at: usize, _head: u16) -> usize {
                at + $n
            }

            fn validate(bytes: &'a [u8], at: usize, _head: u16) -> Result<usize> {
                need(bytes, at + $n)?;
                Ok(at + $n)
            }

            fn to_owned(self) -> Self {
                self
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

impl Layout for bool {
    const FIXED_SIZE: Option<usize> = Some(1);
    const ALIGN: usize = 1;
}

impl Wire for bool {
    fn encoded_size(&self) -> usize {
        1
    }

    fn encode(&self, w: &mut Writer) {
        w.push_bool(*self);
    }
}

impl<'a> View<'a> for bool {
    type Owned = Self;

    fn read(bytes: &'a [u8], at: usize, _head: u16) -> Self {
        read_bool(bytes, at)
    }

    fn end(_bytes: &'a [u8], at: usize, _head: u16) -> usize {
        at + 1
    }

    fn validate(bytes: &'a [u8], at: usize, _head: u16) -> Result<usize> {
        need(bytes, at + 1)?;
        if bytes[at] > 1 {
            return Err(Error::BadBool);
        }
        Ok(at + 1)
    }

    fn to_owned(self) -> Self {
        self
    }
}

impl Canonical for bool {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        self.cmp(other)
    }
}

impl Layout for Timestamp {
    const FIXED_SIZE: Option<usize> = Some(8);
    const ALIGN: usize = 8;
}

impl Wire for Timestamp {
    fn encoded_size(&self) -> usize {
        8
    }

    fn encode(&self, w: &mut Writer) {
        w.push_i64(self.timestamp_micros());
    }
}

impl<'a> View<'a> for Timestamp {
    type Owned = Self;

    fn read(bytes: &'a [u8], at: usize, _head: u16) -> Self {
        DateTime::from_timestamp_micros(read_i64(bytes, at)).unwrap_or_default()
    }

    fn end(_bytes: &'a [u8], at: usize, _head: u16) -> usize {
        at + 8
    }

    fn validate(bytes: &'a [u8], at: usize, _head: u16) -> Result<usize> {
        need(bytes, at + 8)?;
        match DateTime::<Utc>::from_timestamp_micros(read_i64(bytes, at)) {
            Some(_) => Ok(at + 8),
            None => Err(Error::BadTimestamp),
        }
    }

    fn to_owned(self) -> Self {
        self
    }
}

impl Canonical for Timestamp {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        self.cmp(other)
    }
}

impl Layout for Interval {
    const FIXED_SIZE: Option<usize> = Some(8);
    const ALIGN: usize = 8;
}

impl Wire for Interval {
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

    fn read(bytes: &'a [u8], at: usize, _head: u16) -> Self {
        Self::from_micros(read_i64(bytes, at).max(0) as u64)
    }

    fn end(_bytes: &'a [u8], at: usize, _head: u16) -> usize {
        at + 8
    }

    fn validate(bytes: &'a [u8], at: usize, _head: u16) -> Result<usize> {
        need(bytes, at + 8)?;
        if read_i64(bytes, at) < 0 {
            return Err(Error::BadInterval);
        }
        Ok(at + 8)
    }

    fn to_owned(self) -> Self {
        self
    }
}

impl Canonical for Interval {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        self.cmp(other)
    }
}

impl Layout for Entity {
    const FIXED_SIZE: Option<usize> = Some(8);
    const ALIGN: usize = 8;
}

impl Wire for Entity {
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

    fn read(bytes: &'a [u8], at: usize, _head: u16) -> Self {
        Self::new(
            EntityIndex::from_bits(read_u32(bytes, at)),
            EntityGeneration::from_bits(read_u32(bytes, at + 4)),
        )
    }

    fn end(_bytes: &'a [u8], at: usize, _head: u16) -> usize {
        at + 8
    }

    fn validate(bytes: &'a [u8], at: usize, _head: u16) -> Result<usize> {
        need(bytes, at + 8)?;
        if read_u32(bytes, at) == EntityIndex::NULL.to_bits() {
            return Err(Error::BadEntity);
        }
        Ok(at + 8)
    }

    fn to_owned(self) -> Self {
        self
    }
}

impl Canonical for Entity {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        self.index()
            .cmp(&other.index())
            .then_with(|| self.generation().cmp(&other.generation()))
    }
}

impl Layout for UniversalEntity {
    const FIXED_SIZE: Option<usize> = Some(Self::BYTES);
    const ALIGN: usize = 4;
}

impl Wire for UniversalEntity {
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

    fn read(bytes: &'a [u8], at: usize, _head: u16) -> Self {
        Self::new(
            read_u16(bytes, at),
            read_u16(bytes, at + 2),
            Entity::read(bytes, at + 4, 0),
        )
    }

    fn end(_bytes: &'a [u8], at: usize, _head: u16) -> usize {
        at + Self::BYTES
    }

    fn validate(bytes: &'a [u8], at: usize, _head: u16) -> Result<usize> {
        need(bytes, at + Self::BYTES)?;
        Entity::validate(bytes, at + 4, 0)?;
        Ok(at + Self::BYTES)
    }

    fn to_owned(self) -> Self {
        self
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

impl Layout for Uuid {
    const FIXED_SIZE: Option<usize> = Some(Self::SIZE);
    const ALIGN: usize = 1;
}

impl Wire for Uuid {
    fn encoded_size(&self) -> usize {
        Self::SIZE
    }

    fn encode(&self, w: &mut Writer) {
        w.push_bytes(&self.0);
    }
}

impl<'a> View<'a> for Uuid {
    type Owned = Self;

    fn read(bytes: &'a [u8], at: usize, _head: u16) -> Self {
        let mut value = [0u8; Self::SIZE];
        value.copy_from_slice(&bytes[at..at + Self::SIZE]);
        Self(value)
    }

    fn end(_bytes: &'a [u8], at: usize, _head: u16) -> usize {
        at + Self::SIZE
    }

    fn validate(bytes: &'a [u8], at: usize, _head: u16) -> Result<usize> {
        need(bytes, at + Self::SIZE)?;
        Ok(at + Self::SIZE)
    }

    fn to_owned(self) -> Self {
        self
    }
}

impl Canonical for Uuid {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        self.cmp(other)
    }
}

/// `array<T, N>`: `N` fixed-size items packed at their stride.
impl<T: Layout, const N: usize> Layout for [T; N] {
    const FIXED_SIZE: Option<usize> = Some(fixed_size(T::FIXED_SIZE) * N);
    const ALIGN: usize = T::ALIGN;
}

impl<T: Wire, const N: usize> Wire for [T; N] {
    fn encoded_size(&self) -> usize {
        fixed_size(T::FIXED_SIZE) * N
    }

    fn encode(&self, w: &mut Writer) {
        for item in self {
            item.encode(w);
        }
    }
}

impl<'a, T: View<'a>, const N: usize> View<'a> for [T; N] {
    type Owned = [T::Owned; N];

    fn read(bytes: &'a [u8], at: usize, _head: u16) -> Self {
        let stride = fixed_size(T::FIXED_SIZE);
        core::array::from_fn(|index| T::read(bytes, at + index * stride, 0))
    }

    fn end(_bytes: &'a [u8], at: usize, _head: u16) -> usize {
        at + fixed_size(T::FIXED_SIZE) * N
    }

    fn validate(bytes: &'a [u8], at: usize, _head: u16) -> Result<usize> {
        let stride = fixed_size(T::FIXED_SIZE);
        let end = at + stride * N;
        need(bytes, end)?;
        for index in 0..N {
            T::validate(bytes, at + index * stride, 0)?;
        }
        Ok(end)
    }

    fn to_owned(self) -> [T::Owned; N] {
        self.map(View::to_owned)
    }
}

impl<T: Canonical, const N: usize> Canonical for [T; N] {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        for (a, b) in self.iter().zip(other) {
            match a.canonical_cmp(b) {
                Ordering::Equal => {}
                ord => return ord,
            }
        }
        Ordering::Equal
    }
}

/// `string`: UTF-8 bytes, with the byte length in the head.
impl Layout for String {
    const FIXED_SIZE: Option<usize> = None;
    const ALIGN: usize = 1;
}

impl Wire for String {
    fn encoded_size(&self) -> usize {
        self.len()
    }

    fn encode(&self, w: &mut Writer) {
        w.push_bytes(self.as_bytes());
    }

    fn head(&self) -> usize {
        self.len()
    }
}

impl Layout for &str {
    const FIXED_SIZE: Option<usize> = None;
    const ALIGN: usize = 1;
}

impl<'a> View<'a> for &'a str {
    type Owned = String;

    fn read(bytes: &'a [u8], at: usize, head: u16) -> Self {
        let end = at + head as usize;
        core::str::from_utf8(&bytes[at..end]).unwrap_or_default()
    }

    fn end(_bytes: &'a [u8], at: usize, head: u16) -> usize {
        at + head as usize
    }

    fn validate(bytes: &'a [u8], at: usize, head: u16) -> Result<usize> {
        let end = at + head as usize;
        need(bytes, end)?;
        core::str::from_utf8(&bytes[at..end]).map_err(|_| Error::BadUtf8)?;
        Ok(end)
    }

    fn to_owned(self) -> String {
        self.to_string()
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

/// `bytes`: raw bytes, with the byte length in the head. Their owned form is
/// `Vec<u8>`, which encodes identically as `vec<u8>`.
impl Layout for &[u8] {
    const FIXED_SIZE: Option<usize> = None;
    const ALIGN: usize = 1;
}

impl<'a> View<'a> for &'a [u8] {
    type Owned = Vec<u8>;

    fn read(bytes: &'a [u8], at: usize, head: u16) -> Self {
        &bytes[at..at + head as usize]
    }

    fn end(_bytes: &'a [u8], at: usize, head: u16) -> usize {
        at + head as usize
    }

    fn validate(bytes: &'a [u8], at: usize, head: u16) -> Result<usize> {
        let end = at + head as usize;
        need(bytes, end)?;
        Ok(end)
    }

    fn to_owned(self) -> Vec<u8> {
        self.to_vec()
    }
}

impl Canonical for &[u8] {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        (*self).cmp(*other)
    }
}
