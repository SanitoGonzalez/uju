use std::any::Any;
use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

use crate::ecs::component::Component;
use crate::ecs::entity::Entity;
use crate::ecs::hash::FoldBuildHasher;
use crate::ecs::storage::{Storage, table::Table};
use crate::ecs::world::World;

/// What a table keeps for its component's hooks until the next flush: the
/// entities that gained a value, when the component has an `on_insert`
/// hook, and the values removed, when it has an `on_remove` one.
///
/// A table whose component has no hook keeps nothing, and pays one check
/// per insert and removal. A storage built on a
/// [`SparseSet`](crate::ecs::storage::sparse_set::SparseSet) logs through
/// the set's; another calls [`inserted`](Self::inserted) and
/// [`removed`](Self::removed) itself.
pub struct HookLog<T>(Option<Box<Log<T>>>);

struct Log<T> {
    inserts: bool,
    removes: bool,
    inserted: Vec<Entity>,
    removed: Vec<(Entity, T)>,
    /// Shared by every table of the world, set when anything is logged.
    pending: Pending,
}

impl<T> HookLog<T> {
    pub const fn new() -> Self {
        Self(None)
    }

    /// Logs that `entity` gained a value.
    #[inline]
    pub fn inserted(&mut self, entity: Entity) {
        if let Some(log) = &mut self.0 {
            log.inserted(entity);
        }
    }

    /// Logs that `entity` lost `value`, and returns the value unless an
    /// `on_remove` hook keeps it.
    #[inline]
    pub fn removed(&mut self, entity: Entity, value: T) -> Option<T> {
        match &mut self.0 {
            Some(log) => log.removed(entity, value),
            None => Some(value),
        }
    }

    /// Logs an insert into `entity` that replaced `previous`, if any, as a
    /// removal then an insert, and returns `previous` unless an `on_remove`
    /// hook keeps it.
    #[inline]
    pub fn replaced(&mut self, entity: Entity, previous: Option<T>) -> Option<T> {
        match &mut self.0 {
            Some(log) => log.replaced(entity, previous),
            None => previous,
        }
    }

    /// Starts logging what the hooks of the component need. Only changes
    /// made after it are logged.
    pub(crate) fn enable(&mut self, inserts: bool, removes: bool, pending: &Pending) {
        let log = self.0.get_or_insert_with(|| {
            Box::new(Log {
                inserts: false,
                removes: false,
                inserted: Vec::new(),
                removed: Vec::new(),
                pending: pending.clone(),
            })
        });
        log.inserts |= inserts;
        log.removes |= removes;
    }

    /// Swaps the log with `inserted` and `removed`, which are empty, so
    /// their capacity is reused.
    fn swap(&mut self, inserted: &mut Vec<Entity>, removed: &mut Vec<(Entity, T)>) {
        if let Some(log) = &mut self.0 {
            std::mem::swap(&mut log.inserted, inserted);
            std::mem::swap(&mut log.removed, removed);
        }
    }
}

// out of line, so a table without hooks inlines only the check
impl<T> Log<T> {
    #[inline(never)]
    fn inserted(&mut self, entity: Entity) {
        if self.inserts {
            self.inserted.push(entity);
            self.pending.set();
        }
    }

    #[inline(never)]
    fn removed(&mut self, entity: Entity, value: T) -> Option<T> {
        if !self.removes {
            return Some(value);
        }
        self.removed.push((entity, value));
        self.pending.set();
        None
    }

    #[inline(never)]
    fn replaced(&mut self, entity: Entity, previous: Option<T>) -> Option<T> {
        let previous = previous.and_then(|value| self.removed(entity, value));
        self.inserted(entity);
        previous
    }
}

impl<T> Default for HookLog<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

/// Whether any table of a world logged something for a hook since the
/// last flush ran its hooks.
#[derive(Clone, Default)]
pub(crate) struct Pending(Rc<Cell<bool>>);

impl Pending {
    #[inline]
    fn set(&self) {
        self.0.set(true);
    }

    #[inline]
    pub(crate) fn get(&self) -> bool {
        self.0.get()
    }

    #[inline]
    pub(crate) fn take(&self) -> bool {
        self.0.replace(false)
    }
}

type OnInsert = Box<dyn FnMut(&World, &[Entity])>;
type OnRemove<C> = Box<dyn FnMut(&World, &mut Vec<(Entity, C)>)>;

/// One component's hooks, in order of registration, and the batch of the
/// flush's round being run.
pub(crate) struct ComponentHooks<C: Component> {
    pub(crate) on_insert: Vec<OnInsert>,
    pub(crate) on_remove: Vec<OnRemove<C>>,
    inserted: Vec<Entity>,
    removed: Vec<(Entity, C)>,
}

impl<C: Component> ComponentHooks<C> {
    pub(crate) fn new() -> Self {
        Self {
            on_insert: Vec::new(),
            on_remove: Vec::new(),
            inserted: Vec::new(),
            removed: Vec::new(),
        }
    }
}

/// A component's hooks without its type, as the flush drives them.
pub(crate) trait Hooks {
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Takes the component's log out of its table.
    fn take(&mut self, table: &mut dyn Table);

    /// Hands the values taken to the `on_remove` hooks, then drops what
    /// they left.
    fn run_on_remove(&mut self, world: &World);

    /// Hands the entities taken that still hold the component, each once,
    /// to the `on_insert` hooks.
    fn run_on_insert(&mut self, world: &World);
}

impl<C: Component> Hooks for ComponentHooks<C> {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn take(&mut self, table: &mut dyn Table) {
        let table = table.as_any_mut().downcast_mut::<C::Storage>().unwrap();
        table.hook_log().swap(&mut self.inserted, &mut self.removed);
    }

    fn run_on_remove(&mut self, world: &World) {
        if self.removed.is_empty() {
            return;
        }
        for hook in &mut self.on_remove {
            hook(world, &mut self.removed);
        }
        self.removed.clear();
    }

    fn run_on_insert(&mut self, world: &World) {
        if self.inserted.is_empty() {
            return;
        }
        {
            let table = world.view::<C>();
            let mut seen = HashSet::<Entity, FoldBuildHasher>::default();
            self.inserted
                .retain(|&entity| Table::contains(&*table, entity) && seen.insert(entity));
        }
        if !self.inserted.is_empty() {
            for hook in &mut self.on_insert {
                hook(world, &self.inserted);
            }
        }
        self.inserted.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::ecs::query::Query;
    use crate::ecs::schedule::{Schedule, Update};
    use crate::ecs::storage::tracked::Tracked;

    #[derive(crate::ecs::Component, Debug, Clone, PartialEq)]
    struct Collider(u32);

    #[derive(crate::ecs::Component, Debug, PartialEq)]
    #[component(storage = Tracked<Self>)]
    struct Body(u32);

    #[derive(crate::ecs::Component, Debug, PartialEq)]
    struct Plain(u32);

    type Calls<T> = Rc<RefCell<Vec<T>>>;
    type Batches<T> = Calls<Vec<T>>;

    /// A world whose `Collider` and `Body` hooks record what they get.
    fn world() -> (World, Batches<Entity>, Batches<(Entity, Body)>) {
        crate::ecs::init();
        let mut world = World::new();
        let (inserted, removed) = (Calls::default(), Calls::default());
        {
            let inserted = inserted.clone();
            world.on_insert::<Collider>(move |_: &World, added: &[Entity]| {
                inserted.borrow_mut().push(added.to_vec());
            });
        }
        {
            let removed = removed.clone();
            world.on_remove::<Body>(move |_: &World, values: &mut Vec<(Entity, Body)>| {
                removed.borrow_mut().push(std::mem::take(values));
            });
        }
        (world, inserted, removed)
    }

    fn take<T>(calls: &Calls<T>) -> Vec<T> {
        std::mem::take(&mut *calls.borrow_mut())
    }

    #[test]
    fn hooks_run_at_flush() {
        let (world, inserted, removed) = world();
        let entities = (0..4).map(|_| world.spawn()).collect::<Vec<_>>();

        world
            .view_mut::<Collider>()
            .insert(entities[2], Collider(2));
        world.commands().insert(entities[0], Collider(0));
        world.spawn_with((Plain(0), Collider(1)));
        let spawned = world.view::<Plain>().entities()[0];
        world.view_mut::<Body>().insert(entities[3], Body(3));
        assert!(take(&inserted).is_empty());

        world.flush();
        assert_eq!(take(&inserted), [vec![entities[2], spawned, entities[0]]]);

        // a view's change alone is enough for the flush
        world.view_mut::<Body>().remove(entities[3]);
        assert!(take(&removed).is_empty());
        world.flush();
        assert_eq!(take(&removed), [vec![(entities[3], Body(3))]]);

        // nothing changed: no call
        world.flush();
        assert!(take(&inserted).is_empty());
        assert!(take(&removed).is_empty());
    }

    #[test]
    fn hooks_on_insert_skips_lost() {
        let (world, inserted, _) = world();
        let entities = (0..5).map(|_| world.spawn()).collect::<Vec<_>>();
        {
            let mut colliders = world.view_mut::<Collider>();
            // lost again
            colliders.insert(entities[0], Collider(0));
            colliders.remove(entities[0]);
            // lost and gained again: once
            colliders.insert(entities[1], Collider(1));
            colliders.remove(entities[1]);
            colliders.insert(entities[1], Collider(1));
            // replaced: once
            colliders.insert(entities[2], Collider(2));
            colliders.insert(entities[2], Collider(2));
            colliders.insert(entities[3], Collider(3));
            colliders.insert(entities[4], Collider(4));
        }
        world.despawn(entities[3]);
        world.commands().remove::<Collider>(entities[4]);
        world.flush();
        assert_eq!(take(&inserted), [vec![entities[1], entities[2]]]);
    }

    #[test]
    fn hooks_on_remove_gets_every_value() {
        let (world, _, removed) = world();
        let entities = (0..6).map(|_| world.spawn()).collect::<Vec<_>>();
        for (index, &entity) in entities.iter().enumerate() {
            world.view_mut::<Body>().insert(entity, Body(index as u32));
        }
        world.flush();

        // the hook takes the value, so the view hands back none
        assert_eq!(world.view_mut::<Body>().remove(entities[0]), None);
        // an insert over a value is a removal then an insert
        assert_eq!(world.view_mut::<Body>().insert(entities[1], Body(10)), None);
        world.view_mut::<Body>().insert(entities[1], Body(11));
        world.commands().remove::<Body>(entities[2]);
        world.commands().insert(entities[3], Body(13));
        world.despawn(entities[4]);
        world.commands().despawn(entities[5]);
        // added and removed before the flush: still handed over
        let late = world.spawn();
        world.view_mut::<Body>().insert(late, Body(20));
        world.view_mut::<Body>().remove(late);
        world.flush();

        assert_eq!(
            take(&removed),
            [vec![
                (entities[0], Body(0)),
                (entities[1], Body(1)),
                (entities[1], Body(10)),
                (entities[4], Body(4)),
                (late, Body(20)),
                (entities[2], Body(2)),
                (entities[3], Body(3)),
                (entities[5], Body(5)),
            ]]
        );
        assert_eq!(world.view::<Body>().get(entities[1]), Some(&Body(11)));

        // a table without an `on_remove` hook keeps nothing
        let e = world.spawn();
        world.view_mut::<Plain>().insert(e, Plain(1));
        assert_eq!(
            world.view_mut::<Plain>().insert(e, Plain(2)),
            Some(Plain(1))
        );
        assert_eq!(world.view_mut::<Plain>().remove(e), Some(Plain(2)));
    }

    #[test]
    fn hooks_in_order_of_registration() {
        crate::ecs::init();
        let mut world = World::new();
        let calls = Calls::default();
        for name in ["first", "second"] {
            let calls = calls.clone();
            world.on_remove::<Body>(move |_: &World, values: &mut Vec<(Entity, Body)>| {
                calls.borrow_mut().push((name, values.len()));
                // what a hook leaves goes to the next
                values.pop();
            });
        }
        for name in ["insert a", "insert b"] {
            let calls = calls.clone();
            world.on_insert::<Body>(move |_: &World, added: &[Entity]| {
                calls.borrow_mut().push((name, added.len()));
            });
        }
        {
            let calls = calls.clone();
            world.on_insert::<Collider>(move |_: &World, added: &[Entity]| {
                calls.borrow_mut().push(("collider", added.len()));
            });
        }

        let (e0, e1) = (world.spawn(), world.spawn());
        world.view_mut::<Body>().insert(e0, Body(0));
        world.view_mut::<Body>().insert(e1, Body(1));
        world.flush();
        take(&calls);

        // every `on_remove` runs before any `on_insert`
        world.view_mut::<Collider>().insert(e0, Collider(0));
        world.view_mut::<Body>().insert(e0, Body(2));
        world.view_mut::<Body>().remove(e1);
        world.flush();
        let mut calls = take(&calls);
        let inserts = calls.split_off(2);
        assert_eq!(calls, [("first", 2), ("second", 1)]);
        assert_eq!(inserts.len(), 3);
        let at = |name| inserts.iter().position(|&(n, _)| n == name).unwrap();
        assert!(at("insert a") < at("insert b"));
    }

    #[test]
    fn hooks_change_the_world() {
        crate::ecs::init();
        let mut world = World::new();
        // a collider's body is built by its hook, deferred or at once
        world.on_insert::<Collider>(|world: &World, added: &[Entity]| {
            for &entity in added {
                let id = world.view::<Collider>().get(entity).unwrap().0;
                if id.is_multiple_of(2) {
                    world.commands().insert(entity, Body(id));
                } else {
                    world.view_mut::<Body>().insert(entity, Body(id));
                }
            }
        });
        let freed = Calls::default();
        {
            let freed = freed.clone();
            world.on_remove::<Body>(move |world: &World, values: &mut Vec<(Entity, Body)>| {
                for (entity, body) in values.drain(..) {
                    freed.borrow_mut().push(body.0);
                    if world.is_alive(entity) {
                        world.commands().remove::<Collider>(entity);
                    }
                }
            });
        }

        let e0 = world.spawn_with(Collider(0));
        let e1 = world.spawn_with(Collider(1));
        world.flush();
        assert_eq!(world.view::<Body>().get(e0), Some(&Body(0)));
        assert_eq!(world.view::<Body>().get(e1), Some(&Body(1)));

        // the flush runs until quiet: the removal's command applies in it
        world.view_mut::<Body>().remove(e1);
        world.despawn(e0);
        world.flush();
        assert_eq!(*freed.borrow(), [1, 0]);
        assert!(!world.view::<Collider>().contains(e1));
        assert!(world.commands().is_empty());
    }

    #[test]
    fn hooks_not_retroactive() {
        crate::ecs::init();
        let mut world = World::new();
        let e = world.spawn_with((Collider(0), Body(0)));
        let calls = Calls::default();
        {
            let calls = calls.clone();
            world.on_insert::<Collider>(move |_: &World, added: &[Entity]| {
                calls.borrow_mut().push(added.to_vec());
            });
        }
        world.on_remove::<Body>(|_: &World, _: &mut Vec<(Entity, Body)>| {
            panic!("no body was removed after registering");
        });
        world.flush();
        assert!(take(&calls).is_empty());

        let f = world.spawn_with(Collider(1));
        world.flush();
        assert_eq!(take(&calls), [vec![f]]);
        assert!(world.view::<Body>().contains(e));
    }

    #[test]
    fn hooks_nested_flush_applies_commands() {
        crate::ecs::init();
        let mut world = World::new();
        world.on_insert::<Collider>(|world: &World, added: &[Entity]| {
            for &entity in added {
                world.commands().insert(entity, Plain(1));
            }
            // the outer flush runs hooks, this one only applies commands
            world.flush();
            for &entity in added {
                assert!(world.view::<Plain>().contains(entity));
            }
        });
        let e = world.spawn_with(Collider(0));
        world.flush();
        assert!(world.view::<Plain>().contains(e));
    }

    #[test]
    fn hooks_run_after_each_system() {
        let (world, inserted, _) = world();
        let mut schedule = Schedule::new();
        let seen = inserted.clone();
        schedule
            .system(Update, |world: &World| {
                let e = world.spawn();
                world.view_mut::<Collider>().insert(e, Collider(0));
            })
            .system(Update, move |_: &World| {
                assert_eq!(seen.borrow().len(), 1);
            });
        schedule.run(&world);
        assert_eq!(take(&inserted).len(), 1);
        let mut query = Query::<Collider>::new(&world);
        assert_eq!(query.iter().count(), 1);
    }
}
