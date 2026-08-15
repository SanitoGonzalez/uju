use crate::ecs::component::Component;
use crate::ecs::entity::Entity;
use crate::ecs::storage::sparse_array::SparseArray;

pub struct SparseSet<T: Component> {
    sparse: SparseArray<1024>,
    dense: Vec<Entity>,
    data: Vec<T>,
}

impl<T: Component> SparseSet<T> {
    pub fn new() -> Self {
        Self {
            sparse: SparseArray::new(),
            dense: Vec::new(),
            data: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.sparse.clear();
        self.dense.clear();
        self.data.clear();
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.dense.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.dense.is_empty()
    }

    pub fn insert(&mut self, entity: Entity, component: T) {
        match self.sparse.get(entity.index_sparse()) {
            Some(index) => {
                self.dense[index] = entity;
                self.data[index] = component;
            }
            None => {
                self.sparse.insert(entity.index_sparse(), self.dense.len());
                self.dense.push(entity);
                self.data.push(component);
            }
        }
    }

    pub fn remove(&mut self, entity: Entity) -> Option<T> {
        let Some(index) = self.index_of(entity) else {
            return None;
        };

        self.sparse.remove(entity.index_sparse());
        self.dense.swap_remove(index);
        let component = self.data.swap_remove(index);

        if let Some(&swapped) = self.dense.get(index) {
            self.sparse.insert(swapped.index_sparse(), index);
        }

        Some(component)
    }

    #[inline]
    pub fn contains(&self, entity: Entity) -> bool {
        self.index_of(entity).is_some()
    }

    #[inline]
    pub fn get(&self, entity: Entity) -> Option<&T> {
        self.index_of(entity).and_then(|index| self.data.get(index))
    }

    #[inline]
    pub fn get_mut(&mut self, entity: Entity) -> Option<&mut T> {
        self.index_of(entity)
            .and_then(|index| self.data.get_mut(index))
    }

    #[inline]
    fn index_of(&self, entity: Entity) -> Option<usize> {
        let index = self.sparse.get(entity.index_sparse())?;
        (self.dense[index] == entity).then_some(index)
    }
    
    pub fn entities(&self) -> impl Iterator<Item = &Entity> {
        self.dense.iter()
    }

    pub fn components(&self) -> impl Iterator<Item = &T> {
        self.data.iter()
    }

    pub fn components_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.data.iter_mut()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Entity, &T)> {
        self.dense.iter().zip(self.data.iter())
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&Entity, &mut T)> {
        self.dense.iter().zip(self.data.iter_mut())
    }
}

impl<T: Component> Default for SparseSet<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::Component;
    use crate::ecs::entity::{Entity, EntityGeneration, EntityIndex};

    #[derive(Component, Debug, PartialEq)]
    struct Foo(i32);

    #[test]
    fn sparse_set() {
        let mut set = SparseSet::default();
        let e0 = Entity::new(EntityIndex::from_bits(0), EntityGeneration::from_bits(0));
        let e1 = Entity::new(EntityIndex::from_bits(1), EntityGeneration::from_bits(0));
        let e2 = Entity::new(EntityIndex::from_bits(2), EntityGeneration::from_bits(0));
        let e3 = Entity::new(EntityIndex::from_bits(3), EntityGeneration::from_bits(0));
        let e4 = Entity::new(EntityIndex::from_bits(4), EntityGeneration::from_bits(0));

        set.insert(e1, Foo(1));
        set.insert(e2, Foo(2));
        set.insert(e3, Foo(3));

        assert_eq!(set.get(e0), None);
        assert_eq!(set.get(e1), Some(&Foo(1)));
        assert_eq!(set.get(e2), Some(&Foo(2)));
        assert_eq!(set.get(e3), Some(&Foo(3)));
        assert_eq!(set.get(e4), None);

        {
            let iter_results = set.components().collect::<Vec<_>>();
            assert_eq!(iter_results, vec![&Foo(1), &Foo(2), &Foo(3)]);
        }

        assert_eq!(set.remove(e2), Some(Foo(2)));
        assert_eq!(set.remove(e2), None);

        assert_eq!(set.get(e0), None);
        assert_eq!(set.get(e1), Some(&Foo(1)));
        assert_eq!(set.get(e2), None);
        assert_eq!(set.get(e3), Some(&Foo(3)));
        assert_eq!(set.get(e4), None);

        assert_eq!(set.remove(e1), Some(Foo(1)));

        assert_eq!(set.get(e0), None);
        assert_eq!(set.get(e1), None);
        assert_eq!(set.get(e2), None);
        assert_eq!(set.get(e3), Some(&Foo(3)));
        assert_eq!(set.get(e4), None);

        set.insert(e1, Foo(10));

        assert_eq!(set.get(e1), Some(&Foo(10)));

        *set.get_mut(e1).unwrap() = Foo(11);
        assert_eq!(set.get(e1), Some(&Foo(11)));
    }
}
