//! Zero-copy views of the containers: they hold the buffer, the offset of the
//! payload, and the count from the head, and read items on demand.
//!
//! Fixed-size items are packed at a constant stride, so they are indexed
//! directly; variable-size ones are consecutive blocks, so reaching the `n`-th
//! means walking the ones before it.

use core::cmp::Ordering;
use core::marker::PhantomData;

use crate::wire::collections::{Map, Set};
use crate::wire::error::{Error, Result, need};
use crate::wire::traits::{
    Canonical, Entry, Layout, View, element_align, element_end, entry, fixed_size, pad,
    read_element, validate_element, validate_fixed,
};

pub struct VecView<'a, T> {
    bytes: &'a [u8],
    at: usize,
    len: usize,
    item: PhantomData<T>,
}

pub struct SetView<'a, T>(VecView<'a, T>);

pub struct MapView<'a, K, V> {
    bytes: &'a [u8],
    at: usize,
    len: usize,
    key: PhantomData<K>,
    value: PhantomData<V>,
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

impl<'a, T: View<'a>> VecView<'a, T> {
    pub fn len(self) -> usize {
        self.len
    }

    pub fn is_empty(self) -> bool {
        self.len == 0
    }

    /// Offset just past the payload.
    pub fn end(self) -> usize {
        match T::FIXED_SIZE {
            Some(size) => self.at + self.len * size,
            None => self
                .offsets()
                .last()
                .map_or(self.at, |at| element_end::<T>(self.bytes, at)),
        }
    }

    pub fn get(self, index: usize) -> Option<T> {
        if index >= self.len {
            return None;
        }
        match T::FIXED_SIZE {
            Some(size) => Some(read_element(self.bytes, self.at + index * size)),
            None => self.iter().nth(index),
        }
    }

    pub fn iter(self) -> impl Iterator<Item = T> + 'a {
        let bytes = self.bytes;
        self.offsets().map(move |at| read_element(bytes, at))
    }

    /// Offset of each item, walking the blocks when they are variable-size.
    fn offsets(self) -> impl Iterator<Item = usize> + 'a {
        let bytes = self.bytes;
        let align = element_align(T::FIXED_SIZE, T::ALIGN);
        let mut cursor = self.at;
        (0..self.len).map(move |_| {
            let at = pad(cursor, align);
            cursor = element_end::<T>(bytes, at);
            at
        })
    }
}

impl<'a, T: Layout> Layout for VecView<'a, T> {
    const FIXED_SIZE: Option<usize> = None;
    const ALIGN: usize = element_align(T::FIXED_SIZE, T::ALIGN);
}

impl<'a, T: View<'a>> View<'a> for VecView<'a, T> {
    type Owned = Vec<T::Owned>;

    fn read(bytes: &'a [u8], at: usize, head: u16) -> Self {
        Self {
            bytes,
            at,
            len: head as usize,
            item: PhantomData,
        }
    }

    fn end(bytes: &'a [u8], at: usize, head: u16) -> usize {
        Self::read(bytes, at, head).end()
    }

    fn validate(bytes: &'a [u8], at: usize, head: u16) -> Result<usize> {
        validate_items::<T>(bytes, at, head as usize)
    }

    fn to_owned(self) -> Vec<T::Owned> {
        self.iter().map(View::to_owned).collect()
    }
}

impl<'a, T: View<'a> + Canonical> SetView<'a, T> {
    pub fn len(self) -> usize {
        self.0.len()
    }

    pub fn is_empty(self) -> bool {
        self.0.is_empty()
    }

    /// Offset just past the payload.
    pub fn end(self) -> usize {
        self.0.end()
    }

    pub fn get(self, index: usize) -> Option<T> {
        self.0.get(index)
    }

    pub fn iter(self) -> impl Iterator<Item = T> + 'a {
        self.0.iter()
    }

    pub fn contains(self, value: &T) -> bool {
        match T::FIXED_SIZE {
            Some(size) => search(self.0.len, |index| {
                let item: T = read_element(self.0.bytes, self.0.at + index * size);
                item.canonical_cmp(value)
            })
            .is_ok(),
            None => self
                .iter()
                .any(|item| item.canonical_cmp(value) == Ordering::Equal),
        }
    }
}

impl<'a, T: Layout> Layout for SetView<'a, T> {
    const FIXED_SIZE: Option<usize> = None;
    const ALIGN: usize = element_align(T::FIXED_SIZE, T::ALIGN);
}

impl<'a, T: View<'a> + Canonical> View<'a> for SetView<'a, T>
where
    T::Owned: Canonical,
{
    type Owned = Set<T::Owned>;

    fn read(bytes: &'a [u8], at: usize, head: u16) -> Self {
        Self(VecView::read(bytes, at, head))
    }

    fn end(bytes: &'a [u8], at: usize, head: u16) -> usize {
        Self::read(bytes, at, head).end()
    }

    fn validate(bytes: &'a [u8], at: usize, head: u16) -> Result<usize> {
        let end = validate_items::<T>(bytes, at, head as usize)?;
        sorted(Self::read(bytes, at, head).iter())?;
        Ok(end)
    }

    fn to_owned(self) -> Set<T::Owned> {
        Set::from_vec(self.iter().map(View::to_owned).collect())
    }
}

impl<'a, K: View<'a>, V: View<'a>> MapView<'a, K, V> {
    const ENTRY: Entry = entry(
        fixed_size(K::FIXED_SIZE),
        K::ALIGN,
        fixed_size(V::FIXED_SIZE),
        V::ALIGN,
    );

    pub fn len(self) -> usize {
        self.len
    }

    pub fn is_empty(self) -> bool {
        self.len == 0
    }

    /// Offset just past the payload.
    pub fn end(self) -> usize {
        match V::FIXED_SIZE {
            Some(_) => self.at + self.len * Self::ENTRY.size,
            None => {
                let mut cursor = self.at;
                for (_, value) in self.entries() {
                    cursor = element_end::<V>(self.bytes, value);
                }
                cursor
            }
        }
    }

    pub fn key(self, index: usize) -> Option<K> {
        let (key, _) = self.entry_at(index)?;
        Some(read_element(self.bytes, key))
    }

    pub fn value(self, index: usize) -> Option<V> {
        let (_, value) = self.entry_at(index)?;
        Some(read_element(self.bytes, value))
    }

    pub fn iter(self) -> impl Iterator<Item = (K, V)> + 'a {
        let bytes = self.bytes;
        self.entries()
            .map(move |(key, value)| (read_element(bytes, key), read_element(bytes, value)))
    }

    pub fn keys(self) -> impl Iterator<Item = K> + 'a {
        let bytes = self.bytes;
        self.entries().map(move |(key, _)| read_element(bytes, key))
    }

    pub fn values(self) -> impl Iterator<Item = V> + 'a {
        let bytes = self.bytes;
        self.entries()
            .map(move |(_, value)| read_element(bytes, value))
    }

    fn entry_at(self, index: usize) -> Option<(usize, usize)> {
        if index >= self.len {
            return None;
        }
        match V::FIXED_SIZE {
            Some(_) => {
                let at = self.at + index * Self::ENTRY.size;
                Some((at + Self::ENTRY.key, at + Self::ENTRY.value))
            }
            None => self.entries().nth(index),
        }
    }

    /// Offset of each entry's key and value, walking the entries when the
    /// value is variable-size.
    fn entries(self) -> impl Iterator<Item = (usize, usize)> + 'a {
        let bytes = self.bytes;
        let mut cursor = self.at;
        (0..self.len).map(move |_| {
            let at = pad(cursor, Self::ALIGN);
            match V::FIXED_SIZE {
                Some(_) => {
                    cursor = at + Self::ENTRY.size;
                    (at + Self::ENTRY.key, at + Self::ENTRY.value)
                }
                None => {
                    let value = value_at::<K, V>(at);
                    cursor = element_end::<V>(bytes, value);
                    (at, value)
                }
            }
        })
    }
}

impl<'a, K: View<'a> + Canonical, V: View<'a>> MapView<'a, K, V> {
    pub fn get(self, key: &K) -> Option<V> {
        let index = match V::FIXED_SIZE {
            Some(_) => search(self.len, |index| {
                let probe: K = read_element(
                    self.bytes,
                    self.at + index * Self::ENTRY.size + Self::ENTRY.key,
                );
                probe.canonical_cmp(key)
            })
            .ok()?,
            None => self
                .keys()
                .position(|probe| probe.canonical_cmp(key) == Ordering::Equal)?,
        };
        self.value(index)
    }
}

impl<'a, K: Layout, V: Layout> Layout for MapView<'a, K, V> {
    const FIXED_SIZE: Option<usize> = None;
    const ALIGN: usize = {
        let value = element_align(V::FIXED_SIZE, V::ALIGN);
        if K::ALIGN > value { K::ALIGN } else { value }
    };
}

impl<'a, K: View<'a> + Canonical, V: View<'a>> View<'a> for MapView<'a, K, V>
where
    K::Owned: Canonical,
{
    type Owned = Map<K::Owned, V::Owned>;

    fn read(bytes: &'a [u8], at: usize, head: u16) -> Self {
        Self {
            bytes,
            at,
            len: head as usize,
            key: PhantomData,
            value: PhantomData,
        }
    }

    fn end(bytes: &'a [u8], at: usize, head: u16) -> usize {
        Self::read(bytes, at, head).end()
    }

    fn validate(bytes: &'a [u8], at: usize, head: u16) -> Result<usize> {
        let len = head as usize;
        let mut cursor = at;
        for _ in 0..len {
            let entry = pad(cursor, Self::ALIGN);
            match V::FIXED_SIZE {
                Some(_) => {
                    need(bytes, entry + Self::ENTRY.size)?;
                    validate_fixed::<K>(bytes, entry + Self::ENTRY.key)?;
                    validate_fixed::<V>(bytes, entry + Self::ENTRY.value)?;
                    cursor = entry + Self::ENTRY.size;
                }
                None => {
                    validate_fixed::<K>(bytes, entry)?;
                    cursor = validate_element::<V>(bytes, value_at::<K, V>(entry))?;
                }
            }
        }
        sorted(Self::read(bytes, at, head).keys())?;
        Ok(cursor)
    }

    fn to_owned(self) -> Map<K::Owned, V::Owned> {
        Map::from_vec(
            self.iter()
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect(),
        )
    }
}

/// Offset of a variable-size value inside a `map` entry starting at `at`.
fn value_at<'a, K: View<'a>, V: View<'a>>(at: usize) -> usize {
    pad(
        at + fixed_size(K::FIXED_SIZE),
        element_align(V::FIXED_SIZE, V::ALIGN),
    )
}

fn validate_items<'a, T: View<'a>>(bytes: &'a [u8], at: usize, len: usize) -> Result<usize> {
    let align = element_align(T::FIXED_SIZE, T::ALIGN);
    let mut cursor = at;
    if let Some(size) = T::FIXED_SIZE {
        need(bytes, at + len * size)?;
    }
    for _ in 0..len {
        cursor = validate_element::<T>(bytes, pad(cursor, align))?;
    }
    Ok(cursor)
}

/// Checks that `items` are in ascending [`Canonical`] order, as `set` items
/// and `map` keys are on the wire.
fn sorted<T: Canonical>(items: impl Iterator<Item = T>) -> Result<()> {
    let mut previous: Option<T> = None;
    for item in items {
        if let Some(previous) = &previous {
            match previous.canonical_cmp(&item) {
                Ordering::Less => {}
                Ordering::Equal => return Err(Error::Duplicate),
                Ordering::Greater => return Err(Error::Unsorted),
            }
        }
        previous = Some(item);
    }
    Ok(())
}

fn search(len: usize, probe: impl Fn(usize) -> Ordering) -> core::result::Result<usize, usize> {
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
