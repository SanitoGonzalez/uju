use core::cmp::Ordering;

use crate::wire::traits::{
    Canonical, Entry, Layout, Wire, element_align, element_size, encode_element, encode_items,
    entry, fixed_size, items_size, pad,
};
use crate::wire::write::Writer;

/// A `set`: items in [`Canonical`] order, without duplicates — the order they
/// are written in.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Set<T>(Vec<T>);

/// A `map`: entries sorted by key, without duplicate keys.
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
        match self.0.binary_search_by(|probe| probe.canonical_cmp(&value)) {
            Ok(_) => false,
            Err(at) => {
                self.0.insert(at, value);
                true
            }
        }
    }

    pub fn contains(&self, value: &T) -> bool {
        self.0
            .binary_search_by(|probe| probe.canonical_cmp(value))
            .is_ok()
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
        match self.0.binary_search_by(|probe| probe.0.canonical_cmp(&key)) {
            Ok(at) => Some(core::mem::replace(&mut self.0[at].1, value)),
            Err(at) => {
                self.0.insert(at, (key, value));
                None
            }
        }
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        match self.0.binary_search_by(|probe| probe.0.canonical_cmp(key)) {
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

/// `vec<T>`: the item count in the head, then the items.
impl<T: Layout> Layout for Vec<T> {
    const FIXED_SIZE: Option<usize> = None;
    const ALIGN: usize = element_align(T::FIXED_SIZE, T::ALIGN);
}

impl<T: Wire> Wire for Vec<T> {
    fn encoded_size(&self) -> usize {
        items_size(self.iter(), self.len())
    }

    fn encode(&self, w: &mut Writer) {
        encode_items(self.iter(), w);
    }

    fn head(&self) -> usize {
        self.len()
    }
}

impl<T: Layout> Layout for Set<T> {
    const FIXED_SIZE: Option<usize> = None;
    const ALIGN: usize = element_align(T::FIXED_SIZE, T::ALIGN);
}

impl<T: Wire> Wire for Set<T> {
    fn encoded_size(&self) -> usize {
        items_size(self.0.iter(), self.0.len())
    }

    fn encode(&self, w: &mut Writer) {
        encode_items(self.0.iter(), w);
    }

    fn head(&self) -> usize {
        self.0.len()
    }
}

/// `map<K, V>`: the entry count in the head, then the entries. A fixed-size
/// value packs with its key like struct fields; a variable-size one follows
/// the key as a block.
impl<K: Layout, V: Layout> Layout for Map<K, V> {
    const FIXED_SIZE: Option<usize> = None;
    const ALIGN: usize = {
        let value = element_align(V::FIXED_SIZE, V::ALIGN);
        if K::ALIGN > value { K::ALIGN } else { value }
    };
}

impl<K: Wire, V: Wire> Map<K, V> {
    const ENTRY: Entry = entry(
        fixed_size(K::FIXED_SIZE),
        K::ALIGN,
        fixed_size(V::FIXED_SIZE),
        V::ALIGN,
    );
}

impl<K: Wire, V: Wire> Wire for Map<K, V> {
    fn encoded_size(&self) -> usize {
        match V::FIXED_SIZE {
            Some(_) => self.0.len() * Self::ENTRY.size,
            None => self.0.iter().fold(0, |cursor, (_, value)| {
                let at = pad(cursor, Self::ALIGN);
                let value_at = pad(
                    at + fixed_size(K::FIXED_SIZE),
                    element_align(V::FIXED_SIZE, V::ALIGN),
                );
                value_at + element_size(value)
            }),
        }
    }

    fn encode(&self, w: &mut Writer) {
        for (key, value) in &self.0 {
            w.align(Self::ALIGN);
            match V::FIXED_SIZE {
                Some(_) => {
                    let at = w.pos();
                    let entry = Self::ENTRY;
                    if entry.key <= entry.value {
                        w.pad_to(at + entry.key);
                        key.encode(w);
                        w.pad_to(at + entry.value);
                        value.encode(w);
                    } else {
                        w.pad_to(at + entry.value);
                        value.encode(w);
                        w.pad_to(at + entry.key);
                        key.encode(w);
                    }
                    w.pad_to(at + entry.size);
                }
                None => {
                    key.encode(w);
                    w.align(element_align(V::FIXED_SIZE, V::ALIGN));
                    encode_element(value, w);
                }
            }
        }
    }

    fn head(&self) -> usize {
        self.0.len()
    }
}
