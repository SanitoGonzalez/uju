use std::num::NonZeroU32;

use derive_more::{Deref, DerefMut};

pub struct SparseArray<const N: usize> {
    data: Vec<Slot<N>>,
}

impl<const N: usize> SparseArray<N> {
    #[inline]
    pub fn new() -> Self {
        Self { data: Vec::new() }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.data.clear();
    }

    pub fn get(&self, index: usize) -> Option<usize> {
        let value = self.data.get(index / N)?.page.as_deref()?[index % N]?;
        Some(decode(value))
    }

    pub fn insert(&mut self, index: usize, value: usize) {
        let page_index = index / N;
        if page_index >= self.data.len() {
            self.data.resize_with(page_index + 1, Slot::empty);
        }

        let slot = &mut self.data[page_index];
        let page = slot.page.get_or_insert_with(Page::new_boxed);

        let entry = &mut page[index % N];
        if entry.is_none() {
            slot.len += 1;
        }
        *entry = Some(encode(value));
    }

    pub fn remove(&mut self, index: usize) -> Option<usize> {
        let page_index = index / N;
        let slot = self.data.get_mut(page_index)?;
        let page = slot.page.as_deref_mut()?;

        let removed = page[index % N].take()?;

        slot.len -= 1;
        if slot.len == 0 {
            slot.page = None;
            while self.data.last().is_some_and(|slot| slot.page.is_none()) {
                self.data.pop();
            }
        }

        Some(decode(removed))
    }
}

impl<const N: usize> Default for SparseArray<N> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

struct Slot<const N: usize> {
    page: Option<Box<Page<N>>>,
    len: u32,
}

impl<const N: usize> Slot<N> {
    #[inline]
    fn empty() -> Self {
        Self { page: None, len: 0 }
    }
}

#[derive(Deref, DerefMut)]
#[repr(C, align(4096))]
struct Page<const N: usize>([Option<NonZeroU32>; N]);

impl<const N: usize> Page<N> {
    /// Allocates an empty page straight from zeroed memory, skipping the fill.
    #[inline]
    fn new_boxed() -> Box<Self> {
        // SAFETY: `Option<NonZeroU32>` guarantees all-zero bits are `None`, so a zeroed
        // page is a valid page of empty entries.
        unsafe { Box::new_zeroed().assume_init() }
    }
}

// values are stored off by one, so `None` takes the zero bits and an entry stays 4 bytes
#[inline(always)]
fn encode(value: usize) -> NonZeroU32 {
    u32::try_from(value)
        .ok()
        .and_then(|value| NonZeroU32::new(value.wrapping_add(1)))
        .expect("sparse array value overflow")
}

#[inline(always)]
fn decode(value: NonZeroU32) -> usize {
    value.get() as usize - 1
}
