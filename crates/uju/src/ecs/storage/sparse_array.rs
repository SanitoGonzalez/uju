use crate::ecs::entity::{Entity, EntityIndex};

pub struct SparseArray<T, const N: usize> {
    data: Vec<Option<Box<[T; N]>>>,
}

impl<T, const N: usize> SparseArray<T, N> {
    pub fn new() -> Self {
        Self { data: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn clear(&mut self) {
        self.data.clear();
    }
}

impl<const N: usize> SparseArray<EntityIndex, N> {
    #[inline]
    pub fn get(&self, entity: Entity) -> Option<EntityIndex> {
        let index = entity.index_u32() as usize;
        self.data
            .get(index / N)?
            .as_deref()
            .map(|page| page[index % N])
    }

    pub fn insert(&mut self, entity: Entity) {
        let index = entity.index_u32() as usize;
        let page = index / N;
        if page >= self.data.len() {
            self.data.resize_with(page + 1, || None);
        }
        self.data[page].get_or_insert_with(Self::empty_page)[index % N] = entity.index();
    }

    pub fn remove(&mut self, entity: Entity) {}

    fn empty_page() -> Box<[EntityIndex; N]> {
        vec![EntityIndex::NULL; N].try_into().unwrap()
    }
}
