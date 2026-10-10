use std::any::Any;
use std::cell::{Ref, RefCell, RefMut};

use crate::ecs::{
    component::{self, Component},
    entity::{self, Entity},
    replica,
    storage::{
        table::Table,
        view::{View, ViewMut},
    },
    unique::{self, Unique, time::Time},
};

pub struct World {
    pub(crate) entities: RefCell<entity::allocator::Allocator>,
    pub(crate) replicas: RefCell<replica::Registry>,
    pub(crate) tables: Vec<RefCell<Box<dyn Table>>>,
    pub(crate) uniques: Vec<RefCell<Option<Box<dyn Any>>>>,
}

impl World {
    pub fn new() -> Self {
        let world = Self {
            entities: RefCell::new(entity::allocator::Allocator::new()),
            replicas: RefCell::new(replica::Registry::new()),
            tables: component::registrations()
                .into_iter()
                .map(|registration| RefCell::new((registration.new_table)()))
                .collect(),
            uniques: (0..unique::count()).map(|_| RefCell::new(None)).collect(),
        };
        world.insert_unique(Time::new());

        world
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

    pub fn get_unique<U: Unique>(&self) -> Option<Ref<'_, U>> {
        Ref::filter_map(self.uniques[U::id() as usize].borrow(), |slot| {
            slot.as_ref().map(|unique| unique.downcast_ref().unwrap())
        })
        .ok()
    }

    pub fn get_unique_mut<U: Unique>(&self) -> Option<RefMut<'_, U>> {
        RefMut::filter_map(self.uniques[U::id() as usize].borrow_mut(), |slot| {
            slot.as_mut().map(|unique| unique.downcast_mut().unwrap())
        })
        .ok()
    }

    pub fn unique<U: Unique>(&self) -> Ref<'_, U> {
        self.get_unique().expect("unique not inserted")
    }

    pub fn unique_mut<U: Unique>(&self) -> RefMut<'_, U> {
        self.get_unique_mut().expect("unique not inserted")
    }

    pub fn view<C: Component>(&self) -> View<'_, C> {
        View(Ref::map(self.tables[C::id() as usize].borrow(), |table| {
            table.as_any().downcast_ref().unwrap()
        }))
    }

    pub fn view_mut<C: Component>(&self) -> ViewMut<'_, C> {
        ViewMut(RefMut::map(
            self.tables[C::id() as usize].borrow_mut(),
            |table| table.as_any_mut().downcast_mut().unwrap(),
        ))
    }

    pub fn spawn(&self) -> Entity {
        self.entities.borrow_mut().alloc()
    }

    // todo: despawning while table iteration causes `RefCell` panic - need deferred despawn
    pub fn despawn(&self, entity: Entity) -> bool {
        if !self.entities.borrow_mut().dealloc(entity) {
            return false;
        }
        // todo: entity despawn is O(number of components) - optimization is needed
        for table in &self.tables {
            table.borrow_mut().remove(entity);
        }
        true
    }

    pub fn is_alive(&self, entity: Entity) -> bool {
        self.entities.borrow().is_alive(entity)
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

        world.view_mut::<Alpha>().insert(entity, Alpha(1));
        world.view_mut::<Beta>().insert(entity, Beta(2));

        assert_eq!(world.view::<Alpha>().get(entity), Some(&Alpha(1)));
        assert_eq!(world.view::<Beta>().get(entity), Some(&Beta(2)));
    }

    #[test]
    fn world_uniques() {
        crate::ecs::init();
        let world = World::new();

        assert_ne!(Counter::id(), Config::id());
        assert!(world.get_unique::<Counter>().is_none());
        assert!(world.get_unique_mut::<Counter>().is_none());

        assert_eq!(world.insert_unique(Counter(1)), None);
        assert_eq!(world.insert_unique(Config(7)), None);
        assert_eq!(*world.unique::<Counter>(), Counter(1));
        assert_eq!(world.get_unique::<Counter>().as_deref(), Some(&Counter(1)));

        world.unique_mut::<Counter>().0 += 1;
        assert_eq!(*world.unique::<Counter>(), Counter(2));

        world.get_unique_mut::<Counter>().unwrap().0 += 1;
        assert_eq!(*world.unique::<Counter>(), Counter(3));

        assert_eq!(world.insert_unique(Counter(10)), Some(Counter(3)));
        assert_eq!(world.remove_unique::<Counter>(), Some(Counter(10)));
        assert!(world.get_unique::<Counter>().is_none());
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
