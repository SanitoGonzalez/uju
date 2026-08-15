use std::ops::{Deref, DerefMut};

const NULL: u32 = u32::MAX;

pub struct SparseArray<const N: usize> {
    data: Vec<Option<Box<Page<N>>>>,
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
        let value = self.data.get(index / N)?.as_deref()?[index % N];
        if value == NULL {
            None
        } else {
            Some(value as usize)
        }
    }

    pub fn insert(&mut self, index: usize, value: usize) {
        assert!(value < NULL as usize);

        let page = index / N;
        if page >= self.data.len() {
            self.data.resize_with(page + 1, || None);
        }

        self.data[page].get_or_insert_with(|| Box::new(Page::new()))[index % N] = value as u32;
    }

    pub fn remove(&mut self, index: usize) -> Option<usize> {
        let page = self.data.get_mut(index / N)?.as_deref_mut()?;
        let slot = &mut page[index % N];
        match std::mem::replace(slot, NULL) {
            NULL => None,
            removed => Some(removed as usize),
        }
    }
}

impl<const N: usize> Default for SparseArray<N> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

#[repr(C, align(4096))]
struct Page<const N: usize>([u32; N]);

impl<const N: usize> Page<N> {
    #[inline]
    fn new() -> Self {
        Self([NULL; N])
    }
}

impl<const N: usize> Deref for Page<N> {
    type Target = [u32; N];

    #[inline]
    fn deref(&self) -> &<Self as Deref>::Target {
        &self.0
    }
}

impl<const N: usize> DerefMut for Page<N> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
