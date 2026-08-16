pub mod query;

use std::cell::{Ref, RefCell, RefMut};
use std::ops::{Deref, DerefMut};

pub use query::{Mut, Query, Term, With, Without};

use crate::ecs::{
    component::Component,
    entity::Entity,
    unique::{
        Unique,
        view::{Uni, UniMut},
    },
    world::World,
};

/// A system's handle to the [`World`] for the duration of one run.
///
/// All access flows through the transaction - batch [`query`](Tx::query),
/// point [`entity`](Tx::entity) access, uniques, spawns - so systems keep the
/// same shape when the local fast path is later swapped for MVCC snapshot
/// isolation across shards.
///
/// Borrows are dynamic (`RefCell`): access conflicting with a live borrow of
/// the same component or unique (e.g. two queries over `Mut<C>`) panics.
/// Despawns are buffered and applied after the system returns, so they are
/// safe mid-iteration; [`is_alive`](Tx::is_alive) keeps answering `true`
/// until then.
pub struct Tx<'w> {
    world: &'w World,
    despawns: RefCell<Vec<Entity>>,
}

impl<'w> Tx<'w> {
    pub(crate) fn new(world: &'w World) -> Self {
        Self {
            world,
            despawns: RefCell::new(Vec::new()),
        }
    }

    /// Checks out a batch query over the local shard.
    pub fn query<Q: Term>(&self) -> Query<'w, Q> {
        Query::new(self.world)
    }

    /// Point access to one entity; `None` if it is not alive locally.
    pub fn entity(&self, entity: Entity) -> Option<EntityRef<'w>> {
        self.world.is_alive(entity).then_some(EntityRef {
            world: self.world,
            entity,
        })
    }

    pub fn spawn(&self) -> EntityRef<'w> {
        EntityRef {
            world: self.world,
            entity: self.world.spawn(),
        }
    }

    /// Buffers a despawn, applied after the system returns.
    pub fn despawn(&self, entity: Entity) {
        self.despawns.borrow_mut().push(entity);
    }

    pub fn is_alive(&self, entity: Entity) -> bool {
        self.world.is_alive(entity)
    }

    pub fn get_unique<U: Unique>(&self) -> Option<Uni<'w, U>> {
        self.world.get_unique().map(|guard| Uni { guard })
    }

    pub fn get_unique_mut<U: Unique>(&self) -> Option<UniMut<'w, U>> {
        self.world.get_unique_mut().map(|guard| UniMut { guard })
    }

    pub fn unique<U: Unique>(&self) -> Uni<'w, U> {
        self.get_unique().expect("unique not inserted")
    }

    pub fn unique_mut<U: Unique>(&self) -> UniMut<'w, U> {
        self.get_unique_mut().expect("unique not inserted")
    }

    /// Applies buffered commands; called by [`World::run`] after the system
    /// returns, when no borrows are live anymore.
    pub(crate) fn commit(self) {
        let Self { world, despawns } = self;
        for entity in despawns.into_inner() {
            world.despawn(entity);
        }
    }
}

/// Point access to one entity's components.
///
/// [`get`](EntityRef::get)/[`get_mut`](EntityRef::get_mut) hold the
/// component's table borrow while the guard lives; [`insert`](EntityRef::insert)
/// and [`remove`](EntityRef::remove) take the exclusive borrow for the
/// duration of the call. Either panics while a conflicting borrow (a query
/// term or guard over the same component) is live.
#[derive(Clone, Copy)]
pub struct EntityRef<'w> {
    world: &'w World,
    entity: Entity,
}

impl<'w> EntityRef<'w> {
    pub fn entity(&self) -> Entity {
        self.entity
    }

    pub fn contains<C: Component>(&self) -> bool {
        self.world.view::<C>().contains(self.entity)
    }

    pub fn get<C: Component>(&self) -> Option<Comp<'w, C>> {
        Ref::filter_map(self.world.view::<C>().0, |table| table.get(self.entity))
            .ok()
            .map(|guard| Comp { guard })
    }

    pub fn get_mut<C: Component>(&self) -> Option<CompMut<'w, C>> {
        RefMut::filter_map(self.world.view_mut::<C>().0, |table| {
            table.get_mut(self.entity)
        })
        .ok()
        .map(|guard| CompMut { guard })
    }

    /// Inserts a component, returning the displaced one.
    pub fn insert<C: Component>(&self, component: C) -> Option<C> {
        self.world.view_mut::<C>().insert(self.entity, component)
    }

    pub fn remove<C: Component>(&self) -> Option<C> {
        self.world.view_mut::<C>().remove(self.entity)
    }
}

/// Shared borrow of one entity's component, checked out through an
/// [`EntityRef`]. Derefs to `C`.
pub struct Comp<'w, C: Component> {
    guard: Ref<'w, C>,
}

/// Exclusive borrow of one entity's component, checked out through an
/// [`EntityRef`]. Derefs to `C`.
pub struct CompMut<'w, C: Component> {
    guard: RefMut<'w, C>,
}

impl<C: Component> Deref for Comp<'_, C> {
    type Target = C;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl<C: Component> Deref for CompMut<'_, C> {
    type Target = C;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl<C: Component> DerefMut for CompMut<'_, C> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::{Component, Unique};

    #[derive(Component, Debug, PartialEq)]
    struct Position(i32);

    #[derive(Component, Debug, PartialEq)]
    struct Velocity(i32);

    #[derive(Component, Debug, PartialEq)]
    struct Health(i32);

    #[derive(Unique, Debug, PartialEq)]
    struct Gravity(i32);

    #[derive(Unique, Debug, PartialEq)]
    struct Ticks(u32);

    fn movement(tx: &Tx) {
        for (_, (position, velocity)) in tx.query::<(Mut<Position>, Velocity)>().iter() {
            position.0 += velocity.0;
        }
    }

    #[test]
    fn run_fn_system() {
        crate::ecs::init();
        let world = World::new();

        let (still, moving) = world.run(|tx: &Tx| {
            let still = tx.spawn();
            still.insert(Position(0));
            let moving = tx.spawn();
            moving.insert(Position(10));
            moving.insert(Velocity(3));
            (still.entity(), moving.entity())
        });

        world.run(movement);
        world.run(movement);

        world.run(|tx: &Tx| {
            let still = tx.entity(still).unwrap();
            let moving = tx.entity(moving).unwrap();
            assert_eq!(*still.get::<Position>().unwrap(), Position(0));
            assert_eq!(*moving.get::<Position>().unwrap(), Position(16));
        });
    }

    #[test]
    fn run_closure_with_unique() {
        crate::ecs::init();
        let world = World::new();
        world.insert_unique(Gravity(2));

        let entity = world.run(|tx: &Tx| {
            let entity = tx.spawn();
            entity.insert(Velocity(0));
            entity.entity()
        });

        world.run(|tx: &Tx| {
            let gravity = tx.unique::<Gravity>();
            for (_, velocity) in tx.query::<Mut<Velocity>>().iter() {
                velocity.0 -= gravity.0;
            }
        });

        world.run(|tx: &Tx| {
            let entity = tx.entity(entity).unwrap();
            assert_eq!(*entity.get::<Velocity>().unwrap(), Velocity(-2));
        });
    }

    #[test]
    fn run_unique_mut() {
        crate::ecs::init();
        let world = World::new();
        world.insert_unique(Ticks(0));

        fn tick(tx: &Tx) {
            tx.unique_mut::<Ticks>().0 += 1;
        }

        world.run(tick);
        world.run(tick);

        assert_eq!(*world.unique::<Ticks>(), Ticks(2));
    }

    #[test]
    fn point_access() {
        crate::ecs::init();
        let world = World::new();

        let entity = world.run(|tx: &Tx| {
            let entity = tx.spawn();
            assert_eq!(entity.insert(Health(10)), None);
            entity.entity()
        });

        let remaining = world.run(|tx: &Tx| {
            let target = tx.entity(entity).unwrap();
            let mut health = target.get_mut::<Health>().unwrap();
            health.0 -= 3;
            health.0
        });
        assert_eq!(remaining, 7);

        world.run(|tx: &Tx| {
            let target = tx.entity(entity).unwrap();
            assert!(target.contains::<Health>());
            assert_eq!(target.insert(Health(9)), Some(Health(7)));
            assert_eq!(target.remove::<Health>(), Some(Health(9)));
            assert!(!target.contains::<Health>());
            assert!(target.get::<Health>().is_none());
        });
    }

    #[test]
    fn entity_dead_is_none() {
        crate::ecs::init();
        let world = World::new();

        let entity = world.run(|tx: &Tx| tx.spawn().entity());
        world.run(|tx: &Tx| tx.despawn(entity));
        world.run(|tx: &Tx| assert!(tx.entity(entity).is_none()));
    }

    #[test]
    fn despawn_deferred_mid_iteration() {
        crate::ecs::init();
        let world = World::new();

        let (dead, alive) = world.run(|tx: &Tx| {
            let dead = tx.spawn();
            dead.insert(Health(0));
            let alive = tx.spawn();
            alive.insert(Health(5));
            (dead.entity(), alive.entity())
        });

        world.run(|tx: &Tx| {
            for (entity, health) in tx.query::<Health>().iter() {
                if health.0 == 0 {
                    tx.despawn(entity);
                }
            }
            // buffered: applies only after the system returns
            assert!(tx.is_alive(dead));
        });

        assert!(!world.is_alive(dead));
        assert!(world.is_alive(alive));
        assert_eq!(world.view::<Health>().get(dead), None);
        assert_eq!(world.view::<Health>().get(alive), Some(&Health(5)));
    }

    #[test]
    #[should_panic(expected = "already mutably borrowed")]
    fn conflicting_queries_panic() {
        crate::ecs::init();
        let world = World::new();

        world.run(|tx: &Tx| {
            let _mutable = tx.query::<Mut<Position>>();
            let _shared = tx.query::<Position>();
        });
    }

    #[test]
    #[should_panic(expected = "unique not inserted")]
    fn missing_unique_panics() {
        crate::ecs::init();
        let world = World::new();

        world.run(|tx: &Tx| {
            let _ = tx.unique::<Gravity>();
        });
    }
}
