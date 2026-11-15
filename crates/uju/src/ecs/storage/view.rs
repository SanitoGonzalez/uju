use std::cell::{Ref, RefMut};
use std::ops::{Deref, DerefMut};

use crate::ecs::component::Component;
use crate::ecs::entity::{Entity, Registry, Replicas};
use crate::ecs::storage::{
    Storage,
    join::{Entities, IntoJoinable, Joinable},
};

/// Shared borrow of one component's storage, checked out of the [`World`].
///
/// Derefs to [`C::Storage`](Component::Storage) for direct access (`get`,
/// `contains`, `iter`), and `&view` is a join term:
/// `join((&transforms, &velocities))`.
///
/// The borrow is dynamic (`RefCell`), so it must be dropped before the same
/// component is checked out mutably.
///
/// [`World`]: crate::ecs::world::World
pub struct View<'w, C: Component>(pub(crate) Ref<'w, C::Storage>);

/// Exclusive borrow of one component's storage, checked out of the [`World`].
///
/// Derefs to [`C::Storage`](Component::Storage) including mutation
/// (`insert`, `remove`), and both `&view` (shared) and `&mut view` (mutable)
/// are join terms: `join((&mut velocities, &transforms))`.
///
/// When the storage is [replicated](Storage::REPLICATED), its own
/// [`get_mut`](Self::get_mut) and the `&mut view` term skip replicas.
///
/// [`World`]: crate::ecs::world::World
pub struct ViewMut<'w, C: Component> {
    pub(crate) storage: RefMut<'w, C::Storage>,
    pub(crate) registry: &'w Registry,
}

impl<C: Component> ViewMut<'_, C> {
    /// `entity`'s value borrowed mutably, or `None` when it holds none or,
    /// for a replicated storage, is a replica.
    ///
    /// Panics if the replica flags are borrowed mutably, which only the
    /// runtime does, between systems.
    #[inline]
    pub fn get_mut(&mut self, entity: Entity) -> Option<<C::Storage as Storage>::Mut<'_>> {
        if <C::Storage as Storage>::REPLICATED && self.registry.replicas().contains(entity) {
            return None;
        }
        self.storage.get_mut(entity)
    }
}

impl<C: Component> Deref for View<'_, C> {
    type Target = C::Storage;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<C: Component> Deref for ViewMut<'_, C> {
    type Target = C::Storage;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.storage
    }
}

impl<C: Component> DerefMut for ViewMut<'_, C> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.storage
    }
}

impl<'a, C: Component> IntoJoinable<'a> for &'a View<'_, C> {
    type Joinable = &'a C::Storage;

    #[inline]
    fn into_joinable(self) -> Self::Joinable {
        &self.0
    }
}

impl<'a, C: Component> IntoJoinable<'a> for &'a ViewMut<'_, C> {
    type Joinable = &'a C::Storage;

    #[inline]
    fn into_joinable(self) -> Self::Joinable {
        &self.storage
    }
}

impl<'a, C: Component> IntoJoinable<'a> for &'a mut ViewMut<'_, C> {
    type Joinable = SkipReplicas<'a, <C::Storage as Storage>::JoinMut<'a>>;

    #[inline]
    fn into_joinable(self) -> Self::Joinable {
        SkipReplicas {
            replicas: <C::Storage as Storage>::REPLICATED.then(|| self.registry.replicas()),
            join: self.storage.join_mut(),
        }
    }
}

/// A mutable join term over a [`ViewMut`], which skips replicas when the
/// storage is replicated.
pub struct SkipReplicas<'a, J> {
    join: J,
    replicas: Option<Ref<'a, Replicas>>,
}

// SAFETY: delegates to `J`, and only skips some of its probes.
unsafe impl<'a, J: Joinable<'a>> Joinable<'a> for SkipReplicas<'a, J> {
    type Item = J::Item;

    #[inline]
    fn len(&self) -> usize {
        self.join.len()
    }

    #[inline]
    fn entities(&self) -> Entities<'a> {
        self.join.entities()
    }

    #[inline]
    unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
        if self
            .replicas
            .as_ref()
            .is_some_and(|replicas| replicas.contains(entity))
        {
            return None;
        }
        unsafe { self.join.get(entity) }
    }
}

#[cfg(test)]
mod tests {
    use crate::ecs::Component;
    use crate::ecs::entity::{Entity, EntityGeneration, EntityIndex};
    use crate::ecs::storage::join::join;
    use crate::ecs::world::World;

    #[derive(Component, Debug, PartialEq)]
    struct Position(i32);

    #[derive(Component, Debug, PartialEq)]
    struct Speed(i32);

    fn entity(index: u32) -> Entity {
        Entity::new(
            EntityIndex::from_bits(index),
            EntityGeneration::from_bits(1),
        )
    }

    #[test]
    fn view_join() {
        crate::ecs::init();
        let world = World::new();

        for index in 0..4 {
            world
                .view_mut::<Position>()
                .insert(entity(index), Position(0));
        }
        for index in [1, 2] {
            world
                .view_mut::<Speed>()
                .insert(entity(index), Speed(index as i32));
        }

        {
            let mut positions = world.view_mut::<Position>();
            let speeds = world.view::<Speed>();
            for (_, (position, speed)) in join((&mut positions, &speeds)) {
                position.0 += speed.0;
            }
        }

        let positions = world.view::<Position>();
        assert_eq!(positions.get(entity(0)), Some(&Position(0)));
        assert_eq!(positions.get(entity(1)), Some(&Position(1)));
        assert_eq!(positions.get(entity(2)), Some(&Position(2)));
        assert_eq!(positions.get(entity(3)), Some(&Position(0)));
    }

    #[test]
    fn view_mut_as_shared_term() {
        crate::ecs::init();
        let world = World::new();

        world.view_mut::<Position>().insert(entity(0), Position(5));
        world.view_mut::<Speed>().insert(entity(0), Speed(7));

        // A `ViewMut` can join as a shared term without re-borrowing.
        let positions = world.view_mut::<Position>();
        let speeds = world.view::<Speed>();
        let joined = join((&positions, &speeds)).collect::<Vec<_>>();
        assert_eq!(joined, vec![(entity(0), (&Position(5), &Speed(7)))]);
    }

    #[test]
    #[should_panic(expected = "already mutably borrowed")]
    fn view_conflict_panics() {
        crate::ecs::init();
        let world = World::new();

        let _positions = world.view_mut::<Position>();
        let _conflict = world.view::<Position>();
    }
}
