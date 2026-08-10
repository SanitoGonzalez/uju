use crate::component::Component;
use crate::entity::{Entity, EntityIndex};
use crate::storage::sparse_array::SparseArray;

const PAGE_LEN: usize = 4096;

pub struct SparseSet<T: Component> {
    sparse: SparseArray<EntityIndex, PAGE_LEN>,
    dense: Vec<Entity>,
    data: Vec<T>,
}

impl<T: Component> SparseSet<T> {
    pub(crate) fn new() -> Self {
        Self {
            sparse: SparseArray::new(),
            dense: Vec::new(),
            data: Vec::new(),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.sparse.clear();
        self.dense.clear();
        self.data.clear();
    }

    pub(crate) fn insert(&mut self, entity: Entity, component: T) {
        if let Some(index) = self.sparse.get(entity) {
            if index == EntityIndex::NULL {}

            return;
        }
    }

    pub(crate) fn remove(&mut self, entity: Entity) -> bool {
        true
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.dense.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.dense.is_empty()
    }

    #[inline]
    pub fn contains(&self, entity: Entity) -> bool {
        matches!(self.sparse.get(entity), Some(index) if index != EntityIndex::NULL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
}
