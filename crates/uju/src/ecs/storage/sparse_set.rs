use std::any::Any;

use crate::ecs::component::Component;
use crate::ecs::entity::Entity;
use crate::ecs::storage::{
    join::{Entities, Joinable},
    sparse_array::SparseArray,
    table::Table,
};

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

    pub fn insert(&mut self, entity: Entity, component: T) -> Option<T> {
        match self.sparse.get(entity.index_sparse()) {
            Some(index) => {
                self.dense[index] = entity;
                Some(std::mem::replace(&mut self.data[index], component))
            }
            None => {
                self.sparse.insert(entity.index_sparse(), self.dense.len());
                self.dense.push(entity);
                self.data.push(component);
                None
            }
        }
    }

    pub fn remove(&mut self, entity: Entity) -> Option<T> {
        let index = self.index_of(entity)?;

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

    pub fn iter(&self) -> impl Iterator<Item = (Entity, &T)> {
        self.dense.iter().copied().zip(self.data.iter())
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Entity, &mut T)> {
        self.dense.iter().copied().zip(self.data.iter_mut())
    }

    /// Dense entity iterator with a caller-chosen lifetime, for join terms
    /// that cannot express `'a` through their `&self` receiver.
    ///
    /// # Safety
    ///
    /// The set must be borrowed for the whole `'a`, and `dense` must not be
    /// mutated while the iterator is alive.
    #[inline]
    pub(crate) unsafe fn entities_unbound<'a>(&self) -> Entities<'a> {
        unsafe { std::slice::from_raw_parts(self.dense.as_ptr(), self.dense.len()) }
            .iter()
            .copied()
    }

    /// Mutable probe with a caller-chosen lifetime, for join terms.
    ///
    /// # Safety
    ///
    /// The set must be borrowed exclusively for the whole `'a`, and the same
    /// entity must not be probed twice while a returned borrow is alive -
    /// that would alias two `&mut` to one component.
    #[inline]
    pub(crate) unsafe fn get_mut_unbound<'a>(&mut self, entity: Entity) -> Option<&'a mut T> {
        let index = self.index_of(entity)?;
        unsafe { Some(&mut *self.data.as_mut_ptr().add(index)) }
    }
}

impl<T: Component> Default for SparseSet<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Component> Table for SparseSet<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn len(&self) -> usize {
        self.len()
    }
    fn is_empty(&self) -> bool {
        self.is_empty()
    }

    fn remove(&mut self, entity: Entity) -> bool {
        self.remove(entity).is_some()
    }

    fn contains(&self, entity: Entity) -> bool {
        self.contains(entity)
    }

    fn clear(&mut self) {
        self.clear();
    }
}

unsafe impl<'a, T: Component> Joinable<'a> for &'a SparseSet<T> {
    type Item = &'a T;

    #[inline]
    fn len(&self) -> usize {
        self.dense.len()
    }

    #[inline]
    fn entities(&self) -> Entities<'a> {
        let set = *self;
        set.dense.iter().copied()
    }

    #[inline]
    unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
        let set = *self;
        set.get(entity)
    }
}

unsafe impl<'a, T: Component> Joinable<'a> for &'a mut SparseSet<T> {
    type Item = &'a mut T;

    #[inline]
    fn len(&self) -> usize {
        self.dense.len()
    }

    #[inline]
    fn entities(&self) -> Entities<'a> {
        unsafe { self.entities_unbound() }
    }

    #[inline]
    unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
        unsafe { self.get_mut_unbound(entity) }
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
        let e0 = Entity::new(EntityIndex::from_bits(0), EntityGeneration::from_bits(1));
        let e1 = Entity::new(EntityIndex::from_bits(1), EntityGeneration::from_bits(1));
        let e2 = Entity::new(EntityIndex::from_bits(2), EntityGeneration::from_bits(1));
        let e3 = Entity::new(EntityIndex::from_bits(3), EntityGeneration::from_bits(1));
        let e4 = Entity::new(EntityIndex::from_bits(4), EntityGeneration::from_bits(1));

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

        let e1_2nd = Entity::new(EntityIndex::from_bits(1), EntityGeneration::from_bits(3));
        assert_eq!(set.insert(e1_2nd, Foo(42)), Some(Foo(11)));
        assert_eq!(set.get(e1), None);
        assert_eq!(set.get(e1_2nd), Some(&Foo(42)));
    }
}
