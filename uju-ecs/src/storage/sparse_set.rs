use crate::component::Component;
use crate::entity::Entity;

const NULL: u32 = u32::MAX;

pub struct SparseSet<T: Component> {
    sparse: Vec<u32>,
    dense: Vec<Entity>,
    data: Vec<T>,
}

impl<T: Component> SparseSet<T> {
    pub(crate) fn new() -> Self {
        Self {
            sparse: Vec::new(),
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
        if let Some(&dense_index) = self.sparse.get(entity.index()) {
            if dense_index == NULL {}

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
        matches!(self.sparse.get(entity.index()), Some(&dense_index) if dense_index != NULL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
}
