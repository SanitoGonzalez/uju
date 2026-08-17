use core::cmp::Ordering;

use crate::wire::traits::{Canonical, Wire};
use crate::wire::write::Writer;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Set<T>(Vec<T>);

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Map<K, V>(Vec<(K, V)>);

impl<T: Canonical> Set<T> {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn from_vec(mut items: Vec<T>) -> Self {
        items.sort_by(|a, b| a.canonical_cmp(b));
        items.dedup_by(|a, b| a.canonical_cmp(b) == Ordering::Equal);
        Self(items)
    }

    pub fn insert(&mut self, value: T) -> bool {
        match self.0.binary_search_by(|p| p.canonical_cmp(&value)) {
            Ok(_) => false,
            Err(at) => {
                self.0.insert(at, value);
                true
            }
        }
    }

    pub fn contains(&self, value: &T) -> bool {
        self.0.binary_search_by(|p| p.canonical_cmp(value)).is_ok()
    }

    pub fn as_slice(&self) -> &[T] {
        &self.0
    }

    pub fn into_vec(self) -> Vec<T> {
        self.0
    }

    pub fn iter(&self) -> core::slice::Iter<'_, T> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<T: Canonical> FromIterator<T> for Set<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self::from_vec(iter.into_iter().collect())
    }
}

impl<K: Canonical, V> Map<K, V> {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn from_vec(mut items: Vec<(K, V)>) -> Self {
        items.sort_by(|a, b| a.0.canonical_cmp(&b.0));
        items.dedup_by(|a, b| a.0.canonical_cmp(&b.0) == Ordering::Equal);
        Self(items)
    }

    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        match self.0.binary_search_by(|p| p.0.canonical_cmp(&key)) {
            Ok(at) => Some(core::mem::replace(&mut self.0[at].1, value)),
            Err(at) => {
                self.0.insert(at, (key, value));
                None
            }
        }
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        match self.0.binary_search_by(|p| p.0.canonical_cmp(key)) {
            Ok(at) => Some(&self.0[at].1),
            Err(_) => None,
        }
    }

    pub fn as_slice(&self) -> &[(K, V)] {
        &self.0
    }

    pub fn into_vec(self) -> Vec<(K, V)> {
        self.0
    }

    pub fn iter(&self) -> core::slice::Iter<'_, (K, V)> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<K: Canonical, V> FromIterator<(K, V)> for Map<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self::from_vec(iter.into_iter().collect())
    }
}

fn column_size<'a, T: Wire + 'a>(items: impl Iterator<Item = &'a T>, len: usize) -> usize {
    match T::FIXED_SIZE {
        Some(n) => len * n,
        None => len * 2 + items.map(|e| e.encoded_size()).sum::<usize>(),
    }
}

fn encode_column<'a, T: Wire + 'a>(
    w: &mut Writer,
    start: usize,
    items: impl Iterator<Item = &'a T>,
    len: usize,
) {
    if T::FIXED_SIZE.is_some() {
        for element in items {
            element.encode(w);
        }
        return;
    }
    let table = w.pos();
    w.push_zeros(len * 2);
    for (i, element) in items.enumerate() {
        let offset = w.short(w.pos() - start);
        w.put_u16(table + i * 2, offset);
        element.encode(w);
    }
}

impl<T: Wire + Canonical> Wire for Set<T> {
    const FIXED_SIZE: Option<usize> = None;

    fn encoded_size(&self) -> usize {
        2 + column_size(self.0.iter(), self.0.len())
    }

    fn encode(&self, w: &mut Writer) {
        let start = w.pos();
        let count = w.short(self.0.len());
        w.push_u16(count);
        encode_column(w, start, self.0.iter(), self.0.len());
    }
}

impl<T: Canonical> Canonical for Set<T> {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        for (a, b) in self.0.iter().zip(&other.0) {
            match a.canonical_cmp(b) {
                Ordering::Equal => {}
                ord => return ord,
            }
        }
        self.0.len().cmp(&other.0.len())
    }
}

impl<K: Wire + Canonical, V: Wire + Canonical> Wire for Map<K, V> {
    const FIXED_SIZE: Option<usize> = None;

    fn encoded_size(&self) -> usize {
        let keys = column_size(self.0.iter().map(|(k, _)| k), self.0.len());
        let values = column_size(self.0.iter().map(|(_, v)| v), self.0.len());
        4 + keys + values
    }

    fn encode(&self, w: &mut Writer) {
        let start = w.pos();
        let count = w.short(self.0.len());
        w.push_u16(count);
        let values_slot = w.pos();
        w.push_u16(0);
        encode_column(w, start, self.0.iter().map(|(k, _)| k), self.0.len());
        let values_start = w.short(w.pos() - start);
        w.put_u16(values_slot, values_start);
        encode_column(w, start, self.0.iter().map(|(_, v)| v), self.0.len());
    }
}

impl<K: Canonical, V: Canonical> Canonical for Map<K, V> {
    fn canonical_cmp(&self, other: &Self) -> Ordering {
        for ((ak, av), (bk, bv)) in self.0.iter().zip(&other.0) {
            match ak.canonical_cmp(bk).then_with(|| av.canonical_cmp(bv)) {
                Ordering::Equal => {}
                ord => return ord,
            }
        }
        self.0.len().cmp(&other.0.len())
    }
}
