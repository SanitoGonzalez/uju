use std::any::Any;
use std::cell::{Ref, RefCell, RefMut};

use crate::ecs::{
    cell::Cell,
    commands::Commands,
    component::{self, Component},
    entity::{Despawned, Entity, Registry, Uid, UidAllocator},
    storage::{
        table::Table,
        view::{View, ViewMut},
    },
    unique::{self, Unique},
};

/// A shard's entities, component tables and uniques.
///
/// A world is built on its shard's thread and never leaves it.
pub struct World {
    pub(crate) entities: Registry,
    pub(crate) tables: Vec<RefCell<Box<dyn Table>>>,
    pub(crate) uniques: Vec<RefCell<Option<Box<dyn Any>>>>,
    commands: RefCell<Commands>,
    despawns: RefCell<Vec<Despawned>>,
}

impl World {
    /// A world with no [`UidAllocator`]: it [`adopt`](Self::adopt)s uids but cannot
    /// [`promote`](Self::promote) until one is installed.
    pub fn new() -> Self {
        Self {
            entities: Registry::new(),
            tables: component::registrations()
                .into_iter()
                .map(|registration| RefCell::new((registration.new_table)()))
                .collect(),
            uniques: (0..unique::count()).map(|_| RefCell::new(None)).collect(),
            commands: RefCell::new(Commands::default()),
            despawns: RefCell::default(),
        }
    }

    pub fn entities(&self) -> &Registry {
        &self.entities
    }

    pub fn spawn(&self) -> Entity {
        self.entities.spawn()
    }

    /// Frees `entity` and drops its components from every table.
    ///
    /// Panics if any view or query is borrowed at the time.
    pub fn despawn(&self, entity: Entity) -> bool {
        self.despawn_batch(std::slice::from_ref(&entity)) == 1
    }

    /// Despawns every live entity of `entities`, borrowing each table once,
    /// and returns how many it despawned. Dead entities and repeats are
    /// skipped.
    ///
    /// Panics if any view or query is borrowed at the time.
    pub fn despawn_batch(&self, entities: &[Entity]) -> usize {
        let despawned = self
            .entities
            .despawn_batch(entities, &mut self.despawns.borrow_mut());
        if despawned == 0 {
            return 0;
        }

        // a dead entity's probe misses, so the whole slice goes to each table
        for table in &self.tables {
            let mut table = table.borrow_mut();
            if !table.is_empty() {
                table.despawn_batch(entities);
            }
        }
        despawned
    }

    /// The queue of deferred changes, applied by [`flush`](Self::flush). It
    /// is a borrow of its own, so it is available inside a query's loop.
    ///
    /// Panics if the queue is borrowed at the time, such as by a
    /// `Commands` held across this call.
    pub fn commands(&self) -> RefMut<'_, Commands> {
        self.commands.borrow_mut()
    }

    /// Applies the queued [`commands`](Self::commands), in order. Commands
    /// queued while applying, such as by a closure, apply in the same flush.
    ///
    /// Panics if a command conflicts with a live borrow, such as a view of a
    /// table it changes, or if the queue is borrowed at the time.
    pub fn flush(&self) {
        if self.commands.borrow().is_empty() {
            return;
        }

        // keep the queue unborrowed while applying, so commands can queue more
        let mut commands = Commands::default();
        loop {
            std::mem::swap(&mut *self.commands.borrow_mut(), &mut commands);
            if commands.is_empty() {
                break;
            }
            commands.apply(self);
        }
    }

    pub fn is_alive(&self, entity: Entity) -> bool {
        self.entities.is_alive(entity)
    }

    /// See [`Registry::install_uid_allocator`].
    pub fn install_uid_allocator(&self, allocator: UidAllocator) -> Option<UidAllocator> {
        self.entities.install_uid_allocator(allocator)
    }

    /// See [`Registry::uid_allocator_mut`]. The runtime sets the next spare and takes the
    /// retired incarnations through it.
    ///
    /// Panics if the entity allocator is borrowed at the time.
    pub fn uid_allocator_mut(&self) -> Option<RefMut<'_, UidAllocator>> {
        self.entities.uid_allocator_mut()
    }

    /// See [`Registry::uid`].
    pub fn uid(&self, entity: Entity) -> Option<Uid> {
        self.entities.uid(entity)
    }

    /// See [`Registry::promote`].
    pub fn promote(&self, entity: Entity) -> Option<Uid> {
        self.entities.promote(entity)
    }

    /// See [`Registry::resolve`].
    pub fn resolve(&self, uid: Uid) -> Option<Entity> {
        self.entities.resolve(uid)
    }

    /// See [`Registry::adopt`].
    pub fn adopt(&self, uid: Uid) -> Option<Entity> {
        self.entities.adopt(uid)
    }

    /// See [`Registry::cell`].
    pub fn cell(&self, entity: Entity) -> Option<Cell> {
        self.entities.cell(entity)
    }

    /// See [`Registry::is_replica`].
    pub fn is_replica(&self, entity: Entity) -> bool {
        self.entities.is_replica(entity)
    }

    /// The owned entities despawned this tick that had a uid and a cell at
    /// the start of it, in despawn order.
    #[cfg_attr(not(test), expect(dead_code, reason = "read by the encoder"))]
    pub(crate) fn despawns(&self) -> Ref<'_, [Despawned]> {
        Ref::map(self.despawns.borrow(), Vec::as_slice)
    }

    /// Ends the tick: rolls each entity's cell into its cell at the start of
    /// the tick, and clears the despawn log and every table's changes.
    #[cfg_attr(not(test), expect(dead_code, reason = "called by the runtime's tick"))]
    pub(crate) fn end_tick(&mut self) {
        self.entities.end_tick();
        self.despawns.get_mut().clear();
        for table in &mut self.tables {
            table.get_mut().end_tick();
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

    /// Shared access to `C`'s [`Storage`](Component::Storage).
    pub fn view<C: Component>(&self) -> View<'_, C> {
        View(Ref::map(self.tables[C::id() as usize].borrow(), |table| {
            table.as_any().downcast_ref::<C::Storage>().unwrap()
        }))
    }

    /// Mutable access to `C`'s [`Storage`](Component::Storage).
    pub fn view_mut<C: Component>(&self) -> ViewMut<'_, C> {
        ViewMut {
            storage: RefMut::map(self.tables[C::id() as usize].borrow_mut(), |table| {
                table.as_any_mut().downcast_mut::<C::Storage>().unwrap()
            }),
            registry: &self.entities,
        }
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::rc::Rc;

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
        let world = World::new();
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
    fn world_despawn_clears_tables() {
        crate::ecs::init();
        let world = World::new();
        let e0 = world.spawn();
        let e1 = world.spawn();

        world.view_mut::<Alpha>().insert(e0, Alpha(1));
        world.view_mut::<Beta>().insert(e0, Beta(2));
        world.view_mut::<Alpha>().insert(e1, Alpha(3));
        world.install_uid_allocator(UidAllocator::new(NonZeroU32::MIN, NonZeroU32::MAX));
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

    #[derive(Component)]
    struct Shared(Rc<i32>);

    #[derive(Unique)]
    struct SharedUnique(Rc<i32>);

    #[test]
    fn world_holds_non_send() {
        crate::ecs::init();
        let world = World::new();
        let entity = world.spawn();
        let value = Rc::new(7);

        world
            .view_mut::<Shared>()
            .insert(entity, Shared(value.clone()));
        world.insert_unique(SharedUnique(value.clone()));
        assert_eq!(*world.view::<Shared>().get(entity).unwrap().0, 7);
        assert_eq!(*world.unique::<SharedUnique>().0, 7);
        assert_eq!(Rc::strong_count(&value), 3);
    }

    #[test]
    fn world_uids() {
        crate::ecs::init();
        let world = World::new();
        let e0 = world.spawn();

        // nothing to mint from until an allocator is installed
        assert_eq!(world.promote(e0), None);
        assert!(world.uid_allocator_mut().is_none());
        let adopted = Uid::new(NonZeroU32::new(9).unwrap(), 42);
        let e1 = world.adopt(adopted).unwrap();
        assert_eq!(world.resolve(adopted), Some(e1));
        assert_eq!(world.uid(e1), Some(adopted));

        let current = NonZeroU32::new(3).unwrap();
        let spare = NonZeroU32::new(4).unwrap();
        let installed = world.install_uid_allocator(UidAllocator::new(current, spare));
        assert!(installed.is_none());
        let u0 = world.promote(e0).unwrap();
        assert_eq!(u0, Uid::new(current, 0));
        assert_eq!(world.uid(e0), Some(u0));
        assert_eq!(world.resolve(u0), Some(e0));

        let mut allocator = world.uid_allocator_mut().unwrap();
        assert_eq!(allocator.spare(), Some(spare));
        assert!(allocator.take_retired().is_empty());
    }

    #[test]
    fn world_despawn_log() {
        crate::ecs::init();
        let mut world = World::new();
        world.install_uid_allocator(UidAllocator::new(NonZeroU32::MIN, NonZeroU32::MAX));
        let (a, b) = (Cell::new(1), Cell::new(2));
        let entities = (0..5).map(|_| world.spawn()).collect::<Vec<_>>();
        let replica = world
            .adopt(Uid::new(NonZeroU32::new(9).unwrap(), 0))
            .unwrap();
        world.entities().set_replica(replica, true);
        world.entities().set_cell(replica, a);
        let uids = entities
            .iter()
            .map(|&entity| {
                world.entities().set_cell(entity, a);
                world.promote(entity).unwrap()
            })
            .collect::<Vec<_>>();
        world.end_tick();

        // the cell at the start of the tick, though it moved since
        world.entities().set_cell(entities[0], b);
        assert!(world.despawn(entities[0]));
        world.despawn_batch(&[entities[1], replica, entities[1]]);
        world.commands().despawn(entities[2]);
        world.flush();

        let logged = |uid, cell| Despawned { uid, cell };
        assert_eq!(
            *world.despawns(),
            [logged(uids[0], a), logged(uids[1], a), logged(uids[2], a)]
        );
        assert_eq!(world.cell(entities[0]), None);
        assert!(!world.is_replica(replica));
        let mut listed = world.entities().cell_index().entities(a).to_vec();
        listed.sort();
        assert_eq!(listed, [entities[3], entities[4]]);

        world.end_tick();
        assert!(world.despawns().is_empty());
        world.despawn(entities[3]);
        assert_eq!(*world.despawns(), [logged(uids[3], a)]);
    }

    #[test]
    #[should_panic(expected = "unique not inserted")]
    fn world_unique_missing() {
        crate::ecs::init();
        let world = World::new();
        let _ = world.unique::<Counter>();
    }
}
