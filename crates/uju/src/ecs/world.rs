use std::any::Any;
use std::cell::{Ref, RefCell, RefMut};

use crate::cluster::{node, shard};
use crate::ecs::{
    component::{self, Component},
    entity::{Entity, Registry, UniversalAllocator, UniversalId},
    storage::{
        table::Table,
        view::{View, ViewMut},
    },
    unique::{self, Unique, time::Time},
};

pub struct World {
    pub(crate) entities: RefCell<Registry>,
    pub(crate) tables: Vec<RefCell<Box<dyn Table>>>,
    pub(crate) uniques: Vec<RefCell<Option<Box<dyn Any>>>>,
}

impl World {
    /// `node` and `shard` name the minter of this world's [`UniversalId`]s.
    pub fn new(node: node::Id, shard: shard::Id) -> Self {
        // todo: resume from the persisted serial high-water mark instead of 0
        let universal = UniversalAllocator::new(node, shard, 0);
        let world = Self {
            entities: RefCell::new(Registry::new(universal)),
            tables: component::registrations()
                .into_iter()
                .map(|registration| RefCell::new((registration.new_table)()))
                .collect(),
            uniques: (0..unique::count()).map(|_| RefCell::new(None)).collect(),
        };
        world.insert_unique(Time::new());

        world
    }

    pub fn entities(&self) -> Ref<'_, Registry> {
        self.entities.borrow()
    }

    pub fn spawn(&self) -> Entity {
        self.entities.borrow_mut().spawn()
    }

    /// Frees `entity` and drops its components from every table.
    ///
    /// Panics if any view is borrowed at the time.
    pub fn despawn(&self, entity: Entity) -> bool {
        if !self.entities.borrow_mut().despawn(entity) {
            return false;
        }

        for table in &self.tables {
            table.borrow_mut().remove(entity);
        }
        true
    }

    pub fn is_alive(&self, entity: Entity) -> bool {
        self.entities.borrow().is_alive(entity)
    }

    /// See [`Registry::universal`].
    pub fn universal(&self, entity: Entity) -> Option<UniversalId> {
        self.entities.borrow().universal(entity)
    }

    /// See [`Registry::promote`].
    pub fn promote(&self, entity: Entity) -> Option<UniversalId> {
        self.entities.borrow_mut().promote(entity)
    }

    /// See [`Registry::resolve`].
    pub fn resolve(&self, uid: UniversalId) -> Option<Entity> {
        self.entities.borrow().resolve(uid)
    }

    /// See [`Registry::adopt`].
    pub fn adopt(&self, uid: UniversalId) -> Option<Entity> {
        self.entities.borrow_mut().adopt(uid)
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
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let world = World::new(0, 0);
        let entity = world.spawn();

        assert_ne!(Alpha::id(), Beta::id());

        world.view_mut::<Alpha>().insert(entity, Alpha(1));
        world.view_mut::<Beta>().insert(entity, Beta(2));

        assert_eq!(world.view::<Alpha>().get(entity), Some(&Alpha(1)));
        assert_eq!(world.view::<Beta>().get(entity), Some(&Beta(2)));
    }

    #[test]
    fn world_uniques() {
        crate::ecs::init();
        let world = World::new(0, 0);

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
    fn world_despawn_clears_tables() {
        crate::ecs::init();
        let world = World::new(0, 0);
        let e0 = world.spawn();
        let e1 = world.spawn();

        world.view_mut::<Alpha>().insert(e0, Alpha(1));
        world.view_mut::<Beta>().insert(e0, Beta(2));
        world.view_mut::<Alpha>().insert(e1, Alpha(3));
        let u0 = world.promote(e0).unwrap();

        assert!(world.despawn(e0));
        assert!(!world.despawn(e0));
        assert!(!world.is_alive(e0));
        assert_eq!(world.resolve(u0), None);
        assert!(!world.view::<Alpha>().contains(e0));
        assert!(!world.view::<Beta>().contains(e0));
        assert_eq!(world.view::<Alpha>().get(e1), Some(&Alpha(3)));

        // the recycled slot starts empty
        let recycled = world.spawn();
        assert_eq!(recycled.index(), e0.index());
        assert_eq!(world.view::<Alpha>().get(recycled), None);
    }

    #[test]
    #[should_panic(expected = "unique not inserted")]
    fn world_unique_missing() {
        crate::ecs::init();
        let world = World::new(0, 0);
        let _ = world.unique::<Counter>();
    }
}
