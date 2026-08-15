use std::any::Any;
use std::cell::{Ref, RefCell, RefMut};

use crate::ecs::{
    component::{self, Component},
    storage::{sparse_set::SparseSet, table::Table},
    unique::{self, Unique},
};

pub struct World {
    tables: Vec<RefCell<Box<dyn Table>>>,
    uniques: Vec<RefCell<Option<Box<dyn Any>>>>,
}

impl World {
    pub fn new() -> Self {
        Self {
            tables: component::registrations()
                .into_iter()
                .map(|registration| RefCell::new((registration.new_table)()))
                .collect(),
            uniques: (0..unique::count()).map(|_| RefCell::new(None)).collect(),
        }
    }

    pub fn insert_unique<U: Unique>(&self, unique: U) -> Option<U> {
        self.uniques[U::id() as usize]
            .borrow_mut()
            .replace(Box::new(unique))
            .map(|previous| *previous.downcast().unwrap())
    }

    pub fn remove_unique<U: Unique>(&self) -> Option<U> {
        self.uniques[U::id() as usize]
            .borrow_mut()
            .take()
            .map(|unique| *unique.downcast().unwrap())
    }

    pub fn unique<U: Unique>(&self) -> Ref<'_, U> {
        Ref::map(self.uniques[U::id() as usize].borrow(), |slot| {
            slot.as_ref()
                .expect("unique not inserted")
                .downcast_ref()
                .unwrap()
        })
    }

    pub fn unique_mut<U: Unique>(&self) -> RefMut<'_, U> {
        RefMut::map(self.uniques[U::id() as usize].borrow_mut(), |slot| {
            slot.as_mut()
                .expect("unique not inserted")
                .downcast_mut()
                .unwrap()
        })
    }

    pub fn contains_unique<U: Unique>(&self) -> bool {
        self.uniques[U::id() as usize].borrow().is_some()
    }

    fn sparse_set<C: Component>(&self) -> Ref<'_, SparseSet<C>> {
        Ref::map(self.tables[C::id() as usize].borrow(), |table| {
            table.as_any().downcast_ref().unwrap()
        })
    }

    fn sparse_set_mut<C: Component>(&self) -> RefMut<'_, SparseSet<C>> {
        RefMut::map(self.tables[C::id() as usize].borrow_mut(), |table| {
            table.as_any_mut().downcast_mut().unwrap()
        })
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::entity::{Entity, EntityGeneration, EntityIndex};
    use crate::ecs::{Component, Unique};

    #[derive(Component, Debug, PartialEq)]
    struct Alpha(i32);

    #[derive(Component, Debug, PartialEq)]
    struct Beta(i32);

    #[derive(Unique, Debug, PartialEq)]
    struct Counter(u32);

    #[derive(Unique, Debug, PartialEq)]
    struct Config(i32);

    #[test]
    fn world_tables() {
        crate::ecs::init();
        let world = World::new();
        let entity = Entity::new(EntityIndex::from_bits(0), EntityGeneration::from_bits(0));

        assert_ne!(Alpha::id(), Beta::id());

        world.sparse_set_mut::<Alpha>().insert(entity, Alpha(1));
        world.sparse_set_mut::<Beta>().insert(entity, Beta(2));

        assert_eq!(world.sparse_set::<Alpha>().get(entity), Some(&Alpha(1)));
        assert_eq!(world.sparse_set::<Beta>().get(entity), Some(&Beta(2)));
    }

    #[test]
    fn world_uniques() {
        crate::ecs::init();
        let world = World::new();

        assert_ne!(Counter::id(), Config::id());
        assert!(!world.contains_unique::<Counter>());

        assert_eq!(world.insert_unique(Counter(1)), None);
        assert_eq!(world.insert_unique(Config(7)), None);
        assert_eq!(*world.unique::<Counter>(), Counter(1));

        world.unique_mut::<Counter>().0 += 1;
        assert_eq!(*world.unique::<Counter>(), Counter(2));

        assert_eq!(world.insert_unique(Counter(10)), Some(Counter(2)));
        assert_eq!(world.remove_unique::<Counter>(), Some(Counter(10)));
        assert!(!world.contains_unique::<Counter>());
        assert_eq!(*world.unique::<Config>(), Config(7));
    }

    #[test]
    #[should_panic(expected = "unique not inserted")]
    fn world_unique_missing() {
        crate::ecs::init();
        let world = World::new();
        let _ = world.unique::<Counter>();
    }
}
