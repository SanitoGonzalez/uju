use std::cell::{Ref, RefMut};
use std::ops::{Deref, DerefMut};

use crate::ecs::component::Component;
use crate::ecs::entity::Entity;
use crate::ecs::storage::join::{Entities, Joinable};
use crate::ecs::storage::sparse_set::SparseSet;

/// Shared borrow of one component's storage, checked out of the [`World`].
///
/// Derefs to [`SparseSet<C>`] for direct access (`get`, `contains`, `iter`),
/// and `&view` is a join term: `join((&transforms, &velocities))`.
///
/// The borrow is dynamic (`RefCell`), so it must be dropped before the same
/// component is checked out mutably.
///
/// [`World`]: crate::ecs::world::World
pub struct View<'w, C: Component>(pub(crate) Ref<'w, SparseSet<C>>);

/// Exclusive borrow of one component's storage, checked out of the [`World`].
///
/// Derefs to [`SparseSet<C>`] including mutation (`insert`, `remove`,
/// `get_mut`), and both `&view` (shared) and `&mut view` (mutable) are join
/// terms: `join((&mut velocities, &transforms))`.
///
/// [`World`]: crate::ecs::world::World
pub struct ViewMut<'w, C: Component>(pub(crate) RefMut<'w, SparseSet<C>>);

impl<C: Component> Deref for View<'_, C> {
    type Target = SparseSet<C>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<C: Component> Deref for ViewMut<'_, C> {
    type Target = SparseSet<C>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<C: Component> DerefMut for ViewMut<'_, C> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

// SAFETY: delegates to the `&SparseSet` impl's guarantees.
unsafe impl<'a, C: Component> Joinable<'a> for &'a View<'_, C> {
    type Item = &'a C;

    #[inline]
    fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    fn entities(&self) -> Entities<'a> {
        let set: &'a SparseSet<C> = &self.0;
        Joinable::entities(&set)
    }

    #[inline]
    unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
        let set: &'a SparseSet<C> = &self.0;
        set.get(entity)
    }
}

// SAFETY: delegates to the `&SparseSet` impl's guarantees.
unsafe impl<'a, C: Component> Joinable<'a> for &'a ViewMut<'_, C> {
    type Item = &'a C;

    #[inline]
    fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    fn entities(&self) -> Entities<'a> {
        let set: &'a SparseSet<C> = &self.0;
        Joinable::entities(&set)
    }

    #[inline]
    unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
        let set: &'a SparseSet<C> = &self.0;
        set.get(entity)
    }
}

// SAFETY: delegates to the `&mut SparseSet` impl's guarantees.
unsafe impl<'a, C: Component> Joinable<'a> for &'a mut ViewMut<'_, C> {
    type Item = &'a mut C;

    #[inline]
    fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    fn entities(&self) -> Entities<'a> {
        // SAFETY: `self` borrows the view (and through the `RefMut` guard,
        // the set) exclusively for `'a`; joins never mutate `dense`.
        unsafe { self.0.entities_unbound() }
    }

    #[inline]
    unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
        // SAFETY: exclusive borrow for `'a` held by `self`; at-most-once per
        // entity is forwarded from the caller.
        unsafe { self.0.get_mut_unbound(entity) }
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
