use derive_more::{Deref, DerefMut};

const NULL: u32 = u32::MAX;

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
        let value = self.data.get(index / N)?.page.as_deref()?[index % N];
        if value == NULL {
            None
        } else {
            Some(value as usize)
        }
    }

    pub fn insert(&mut self, index: usize, value: usize) {
        assert!(value < NULL as usize);

        let page_index = index / N;
        if page_index >= self.data.len() {
            self.data.resize_with(page_index + 1, Slot::empty);
        }

        let slot = &mut self.data[page_index];
        let page = slot.page.get_or_insert_with(|| Box::new(Page::new()));

        let entry = &mut page[index % N];
        if *entry == NULL {
            slot.len += 1;
        }
        *entry = value as u32;
    }

    pub fn remove(&mut self, index: usize) -> Option<usize> {
        let page_index = index / N;
        let slot = self.data.get_mut(page_index)?;
        let page = slot.page.as_deref_mut()?;

        let removed = match std::mem::replace(&mut page[index % N], NULL) {
            NULL => return None,
            removed => removed,
        };

        slot.len -= 1;
        if slot.len == 0 {
            slot.page = None;
            while self.data.last().is_some_and(|slot| slot.page.is_none()) {
                self.data.pop();
            }
        }

        Some(removed as usize)
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
struct Page<const N: usize>([u32; N]);

impl<const N: usize> Page<N> {
    #[inline]
    fn new() -> Self {
        Self([NULL; N])
    }
}
