use core::cmp::Ordering;
use core::marker::PhantomData;

use crate::wire::error::{Error, Result, need};
use crate::wire::read::read_u16;
use crate::wire::traits::{Canonical, View};

pub struct VecView<'a, T> {
    bytes: &'a [u8],
    len: usize,
    _element: PhantomData<T>,
}

pub struct SetView<'a, T>(VecView<'a, T>);

pub struct MapView<'a, K, V> {
    bytes: &'a [u8],
    len: usize,
    values: usize,
    _key: PhantomData<K>,
    _value: PhantomData<V>,
}

impl<T> Clone for VecView<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for VecView<'_, T> {}

impl<T> Clone for SetView<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for SetView<'_, T> {}

impl<K, V> Clone for MapView<'_, K, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K, V> Copy for MapView<'_, K, V> {}

fn element_at<'a, T: View<'a>>(bytes: &'a [u8], table: usize, index: usize) -> T {
    let at = match T::FIXED_SIZE {
        Some(n) => table + index * n,
        None => read_u16(bytes, table + index * 2) as usize,
    };
    T::read(&bytes[at..])
}

fn validate_column<'a, T: View<'a>>(bytes: &'a [u8], table: usize, len: usize) -> Result<usize> {
    match T::FIXED_SIZE {
        Some(n) => {
            let end = table + len * n;
            need(bytes, end)?;
            for i in 0..len {
                T::validate(&bytes[table + i * n..])?;
            }
            Ok(end)
        }
        None => {
            let mut cursor = table + len * 2;
            need(bytes, cursor)?;
            for i in 0..len {
                if read_u16(bytes, table + i * 2) as usize != cursor {
                    return Err(Error::BadOffset);
                }
                cursor += T::validate(&bytes[cursor..])?;
            }
            Ok(cursor)
        }
    }
}

fn check_sorted<'a, T: View<'a>>(bytes: &'a [u8], table: usize, len: usize) -> Result<()> {
    for i in 1..len {
        let previous: T = element_at(bytes, table, i - 1);
        let current: T = element_at(bytes, table, i);
        match previous.canonical_cmp(&current) {
            Ordering::Less => {}
            Ordering::Equal => return Err(Error::Duplicate),
            Ordering::Greater => return Err(Error::Unsorted),
        }
    }
    Ok(())
}

impl<'a, T: View<'a>> VecView<'a, T> {
    pub fn len(self) -> usize {
        self.len
    }

    pub fn is_empty(self) -> bool {
        self.len == 0
    }

    pub fn get(self, index: usize) -> Option<T> {
        (index < self.len).then(|| element_at(self.bytes, 2, index))
    }

    pub fn iter(self) -> impl Iterator<Item = T> + 'a {
        let bytes = self.bytes;
        (0..self.len).map(move |i| element_at(bytes, 2, i))
    }
}

impl<'a, T: View<'a>> View<'a> for VecView<'a, T> {
    type Owned = Vec<T::Owned>;

    const FIXED_SIZE: Option<usize> = None;

    fn owned(self) -> Vec<T::Owned> {
        self.iter().map(View::owned).collect()
    }

    fn read(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            len: read_u16(bytes, 0) as usize,
            _element: PhantomData,
        }
    }

    fn validate(bytes: &'a [u8]) -> Result<usize> {
        need(bytes, 2)?;
        validate_column::<T>(bytes, 2, read_u16(bytes, 0) as usize)
    }
}

impl<'a, T: View<'a>> Canonical for VecView<'a, T> {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        for i in 0..self.len.min(other.len) {
            let a: T = element_at(self.bytes, 2, i);
            let b: T = element_at(other.bytes, 2, i);
            match a.canonical_cmp(&b) {
                Ordering::Equal => {}
                ord => return ord,
            }
        }
        self.len.cmp(&other.len)
    }
}

impl<'a, T: View<'a>> SetView<'a, T> {
    pub fn len(self) -> usize {
        self.0.len()
    }

    pub fn is_empty(self) -> bool {
        self.0.is_empty()
    }

    pub fn get(self, index: usize) -> Option<T> {
        self.0.get(index)
    }

    pub fn iter(self) -> impl Iterator<Item = T> + 'a {
        self.0.iter()
    }

    pub fn contains(self, value: &T) -> bool {
        self.position(value).is_ok()
    }

    fn position(self, value: &T) -> core::result::Result<usize, usize> {
        binary_search(self.0.len, |i| {
            let probe: T = element_at(self.0.bytes, 2, i);
            probe.canonical_cmp(value)
        })
    }
}

impl<'a, T: View<'a>> View<'a> for SetView<'a, T>
where
    T::Owned: Canonical,
{
    type Owned = crate::wire::collections::Set<T::Owned>;

    const FIXED_SIZE: Option<usize> = None;

    fn owned(self) -> crate::wire::collections::Set<T::Owned> {
        crate::wire::collections::Set::from_vec(self.iter().map(View::owned).collect())
    }

    fn read(bytes: &'a [u8]) -> Self {
        Self(VecView::read(bytes))
    }

    fn validate(bytes: &'a [u8]) -> Result<usize> {
        let end = VecView::<T>::validate(bytes)?;
        check_sorted::<T>(bytes, 2, read_u16(bytes, 0) as usize)?;
        Ok(end)
    }
}

impl<'a, T: View<'a>> Canonical for SetView<'a, T> {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        self.0.canonical_cmp(&other.0)
    }
}

impl<'a, K: View<'a>, V: View<'a>> MapView<'a, K, V> {
    pub fn len(self) -> usize {
        self.len
    }

    pub fn is_empty(self) -> bool {
        self.len == 0
    }

    pub fn key(self, index: usize) -> Option<K> {
        (index < self.len).then(|| element_at(self.bytes, 4, index))
    }

    pub fn value(self, index: usize) -> Option<V> {
        (index < self.len).then(|| element_at(self.bytes, self.values, index))
    }

    pub fn get(self, key: &K) -> Option<V> {
        let index = binary_search(self.len, |i| {
            let probe: K = element_at(self.bytes, 4, i);
            probe.canonical_cmp(key)
        })
        .ok()?;
        self.value(index)
    }

    pub fn iter(self) -> impl Iterator<Item = (K, V)> + 'a {
        let (bytes, values) = (self.bytes, self.values);
        (0..self.len).map(move |i| (element_at(bytes, 4, i), element_at(bytes, values, i)))
    }

    pub fn keys(self) -> impl Iterator<Item = K> + 'a {
        let bytes = self.bytes;
        (0..self.len).map(move |i| element_at(bytes, 4, i))
    }

    pub fn values(self) -> impl Iterator<Item = V> + 'a {
        let (bytes, values) = (self.bytes, self.values);
        (0..self.len).map(move |i| element_at(bytes, values, i))
    }
}

impl<'a, K: View<'a>, V: View<'a>> View<'a> for MapView<'a, K, V>
where
    K::Owned: Canonical,
{
    type Owned = crate::wire::collections::Map<K::Owned, V::Owned>;

    const FIXED_SIZE: Option<usize> = None;

    fn owned(self) -> crate::wire::collections::Map<K::Owned, V::Owned> {
        crate::wire::collections::Map::from_vec(
            self.iter().map(|(k, v)| (k.owned(), v.owned())).collect(),
        )
    }

    fn read(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            len: read_u16(bytes, 0) as usize,
            values: read_u16(bytes, 2) as usize,
            _key: PhantomData,
            _value: PhantomData,
        }
    }

    fn validate(bytes: &'a [u8]) -> Result<usize> {
        need(bytes, 4)?;
        let len = read_u16(bytes, 0) as usize;
        let values = read_u16(bytes, 2) as usize;
        if validate_column::<K>(bytes, 4, len)? != values {
            return Err(Error::BadOffset);
        }
        let end = validate_column::<V>(bytes, values, len)?;
        check_sorted::<K>(bytes, 4, len)?;
        Ok(end)
    }
}

impl<'a, K: View<'a>, V: View<'a>> Canonical for MapView<'a, K, V> {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        for i in 0..self.len.min(other.len) {
            let ak: K = element_at(self.bytes, 4, i);
            let bk: K = element_at(other.bytes, 4, i);
            let av: V = element_at(self.bytes, self.values, i);
            let bv: V = element_at(other.bytes, other.values, i);
            match ak.canonical_cmp(&bk).then_with(|| av.canonical_cmp(&bv)) {
                Ordering::Equal => {}
                ord => return ord,
            }
        }
        self.len.cmp(&other.len)
    }
}

fn binary_search(
    len: usize,
    probe: impl Fn(usize) -> Ordering,
) -> core::result::Result<usize, usize> {
    let (mut lo, mut hi) = (0, len);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match probe(mid) {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Ok(mid),
        }
    }
    Err(lo)
}
