use derive_more::{Deref, DerefMut};

const PAGE_LEN: usize = 512;
const WORD_BITS: usize = u64::BITS as usize;
const PAGE_BITS: usize = PAGE_LEN * WORD_BITS;

pub struct SparseBitset {
    data: Vec<Slot>,
}

impl SparseBitset {
    #[inline]
    pub fn new() -> Self {
        Self { data: Vec::new() }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.data.clear();
    }

    pub fn contains(&self, index: usize) -> bool {
        let Some(page) = self
            .data
            .get(index / PAGE_BITS)
            .and_then(|slot| slot.page.as_deref())
        else {
            return false;
        };

        let bit = index % PAGE_BITS;
        page[bit / WORD_BITS] & (1 << (bit % WORD_BITS)) != 0
    }

    pub fn insert(&mut self, index: usize) {
        let page_index = index / PAGE_BITS;
        if page_index >= self.data.len() {
            self.data.resize_with(page_index + 1, Slot::empty);
        }

        let slot = &mut self.data[page_index];
        let page = slot.page.get_or_insert_with(|| Box::new(Page::new()));

        let bit = index % PAGE_BITS;
        let word = &mut page[bit / WORD_BITS];
        let mask = 1 << (bit % WORD_BITS);

        if *word & mask == 0 {
            *word |= mask;
            slot.len += 1;
        }
    }

    pub fn remove(&mut self, index: usize) -> bool {
        let page_index = index / PAGE_BITS;
        let Some(slot) = self.data.get_mut(page_index) else {
            return false;
        };
        let Some(page) = slot.page.as_deref_mut() else {
            return false;
        };

        let bit = index % PAGE_BITS;
        let word = &mut page[bit / WORD_BITS];
        let mask = 1 << (bit % WORD_BITS);

        if *word & mask == 0 {
            return false;
        }

        *word &= !mask;
        slot.len -= 1;

        if slot.len == 0 {
            slot.page = None;
            while self.data.last().is_some_and(|slot| slot.page.is_none()) {
                self.data.pop();
            }
        }

        true
    }
}

impl Default for SparseBitset {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

struct Slot {
    page: Option<Box<Page>>,
    len: u32,
}

impl Slot {
    #[inline]
    fn empty() -> Self {
        Self { page: None, len: 0 }
    }
}

#[derive(Deref, DerefMut)]
#[repr(C, align(4096))]
struct Page([u64; PAGE_LEN]);

impl Page {
    #[inline]
    fn new() -> Self {
        Self([0; PAGE_LEN])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_bitset() {
        let mut bitset = SparseBitset::default();

        assert!(!bitset.contains(0));
        assert!(!bitset.contains(PAGE_BITS * 3 + 7));

        bitset.insert(0);
        bitset.insert(63);
        bitset.insert(64);
        bitset.insert(PAGE_BITS - 1);
        bitset.insert(PAGE_BITS * 3 + 7);

        assert!(bitset.contains(0));
        assert!(!bitset.contains(1));
        assert!(bitset.contains(63));
        assert!(bitset.contains(64));
        assert!(!bitset.contains(65));
        assert!(bitset.contains(PAGE_BITS - 1));
        assert!(!bitset.contains(PAGE_BITS));
        assert!(bitset.contains(PAGE_BITS * 3 + 7));

        assert!(bitset.remove(64));
        assert!(!bitset.remove(64));
        assert!(!bitset.contains(64));

        assert!(bitset.contains(63));
        assert!(bitset.contains(PAGE_BITS - 1));

        assert!(!bitset.remove(PAGE_BITS * 100));
        assert!(bitset.remove(PAGE_BITS * 3 + 7));
        assert!(!bitset.contains(PAGE_BITS * 3 + 7));

        // the last page emptied out, so it and the gap before it are gone
        assert_eq!(bitset.data.len(), 1);

        bitset.insert(64);
        assert!(bitset.contains(64));

        assert!(bitset.remove(0));
        assert!(bitset.remove(63));
        assert!(bitset.remove(64));
        assert!(!bitset.data.is_empty());

        assert!(bitset.remove(PAGE_BITS - 1));
        assert!(bitset.data.is_empty());

        bitset.insert(PAGE_BITS - 1);
        assert!(bitset.contains(PAGE_BITS - 1));

        bitset.insert(0);
        bitset.insert(64);

        bitset.clear();
        assert!(!bitset.contains(0));
        assert!(!bitset.contains(64));
        assert!(!bitset.contains(PAGE_BITS - 1));
    }
}
