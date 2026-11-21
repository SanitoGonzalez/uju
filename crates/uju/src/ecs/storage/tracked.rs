use std::any::Any;
use std::ops::{Deref, DerefMut};

use crate::ecs::component::Component;
use crate::ecs::entity::Entity;
use crate::ecs::hook::HookLog;
use crate::ecs::storage::{
    Storage, Tracking,
    bits::{Bits, Marker},
    join::{Entities, Joinable},
    sparse_set::SparseSet,
    table::Table,
};

/// A dense table that tracks the tick's changes: a dirty bit and an added
/// bit per entry, and a log of the entities removed.
///
/// Mutable access yields a [`TrackedMut`], which marks the entry dirty on
/// `DerefMut`, so reading through it marks nothing. `insert` over a value
/// marks it dirty, and a new value is marked added and dirty. The bits are
/// cleared at the end of the tick.
///
/// Removing a value added this tick logs nothing, so an add then a remove
/// cancel out. A despawn is not logged either.
pub struct Tracked<T> {
    set: SparseSet<T>,
    dirty: Bits,
    added: Bits,
    removed: Vec<Entity>,
}

impl<T> Tracked<T> {
    pub fn new() -> Self {
        Self {
            set: SparseSet::new(),
            dirty: Bits::default(),
            added: Bits::default(),
            removed: Vec::new(),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.set.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    #[inline]
    pub fn contains(&self, entity: Entity) -> bool {
        self.set.contains(entity)
    }

    #[inline]
    pub fn get(&self, entity: Entity) -> Option<&T> {
        self.set.get(entity)
    }

    /// `entity`'s value, marked dirty when written through.
    #[inline]
    pub fn get_mut(&mut self, entity: Entity) -> Option<TrackedMut<'_, T>> {
        let position = self.set.index_of(entity)?;
        Some(TrackedMut {
            value: self.set.value_mut_at(position),
            dirty: self.dirty.marker(),
            position,
        })
    }

    /// The entities holding a value, in dense order.
    #[inline]
    pub fn entities(&self) -> &[Entity] {
        self.set.entities()
    }

    pub fn iter(&self) -> impl Iterator<Item = (Entity, &T)> {
        self.set.iter()
    }

    pub fn components(&self) -> impl Iterator<Item = &T> {
        self.set.components()
    }

    /// Inserts or replaces `entity`'s value, returning the previous one
    /// unless an `on_remove` hook keeps it. A replaced value is marked
    /// dirty, and a new one added and dirty.
    pub fn insert(&mut self, entity: Entity, component: T) -> Option<T> {
        self.insert_replacing(entity, component).1
    }

    /// [`insert`](Self::insert), also returning whether it replaced a
    /// value, which the value returned does not tell when a hook keeps it.
    pub(crate) fn insert_replacing(&mut self, entity: Entity, component: T) -> (bool, Option<T>) {
        let previous = self.put(entity, component);
        let position = self.set.index_of(entity).unwrap();
        self.dirty.set(position);
        let replaced = previous.is_some();
        if !replaced {
            self.added.set(position);
        }
        (replaced, self.set.hook_log().replaced(entity, previous))
    }

    /// Removes `entity`'s value, returning it unless an `on_remove` hook
    /// keeps it. Logs the removal, unless the value was added this tick.
    pub fn remove(&mut self, entity: Entity) -> Option<T> {
        let position = self.set.index_of(entity)?;
        self.remove_at(entity, position, true)
    }

    /// Whether `entity`'s value was added or marked dirty this tick.
    #[inline]
    pub fn is_changed(&self, entity: Entity) -> bool {
        self.set
            .index_of(entity)
            .is_some_and(|position| self.dirty.get(position))
    }

    /// Whether `entity` gained its value this tick.
    #[inline]
    pub fn is_added(&self, entity: Entity) -> bool {
        self.set
            .index_of(entity)
            .is_some_and(|position| self.added.get(position))
    }

    /// The entities that lost their value this tick, in removal order. Some
    /// may be dead, or hold a value again.
    pub fn removed(&self) -> &[Entity] {
        &self.removed
    }

    /// The entities whose value was added or marked dirty this tick, in
    /// dense order.
    pub fn changed(&self) -> impl Iterator<Item = Entity> + '_ {
        let entities = self.set.entities();
        self.dirty.ones().map(|position| entities[position])
    }

    /// Removes every value, and the tick's changes with them. An
    /// `on_remove` hook gets the values.
    pub fn clear(&mut self) {
        self.set.clear();
        self.dirty = Bits::default();
        self.added = Bits::default();
        self.removed.clear();
    }

    /// `entity`'s dense position, if it holds a value.
    #[inline]
    pub(crate) fn index_of(&self, entity: Entity) -> Option<usize> {
        self.set.index_of(entity)
    }

    /// The value at dense position `position`, which must be in bounds.
    #[inline]
    pub(crate) fn value_at(&self, position: usize) -> &T {
        self.set.value_at(position)
    }

    #[inline]
    pub(crate) fn is_added_at(&self, position: usize) -> bool {
        self.added.get(position)
    }

    /// The dense positions added or marked dirty this tick, in order.
    pub(crate) fn changed_positions(&self) -> impl Iterator<Item = usize> + '_ {
        self.dirty.ones()
    }

    /// The values and a marker of their dirty bits, borrowed apart, for a
    /// table that hands out wrappers of its own.
    #[inline]
    pub(crate) fn split_mut(&mut self) -> (&mut SparseSet<T>, Marker<'_>) {
        (&mut self.set, self.dirty.marker())
    }

    /// `entity`'s value borrowed mutably without marking it, for state
    /// received from another world.
    #[inline]
    pub(crate) fn get_mut_untracked(&mut self, entity: Entity) -> Option<&mut T> {
        self.set.get_mut(entity)
    }

    /// The log kept for the component's hooks.
    #[inline]
    pub fn hook_log(&mut self) -> &mut HookLog<T> {
        self.set.hook_log()
    }

    /// Inserts or replaces a value without marking it, for state received
    /// from another world. A replaced value is overwritten, so only a new
    /// one is logged for the hooks.
    pub(crate) fn insert_untracked(&mut self, entity: Entity, component: T) -> Option<T> {
        let previous = self.put(entity, component);
        if previous.is_none() {
            self.set.hook_log().inserted(entity);
        }
        previous
    }

    /// Removes a value without logging it in the tick's changes. An
    /// `on_remove` hook still gets it.
    pub(crate) fn remove_untracked(&mut self, entity: Entity) -> Option<T> {
        let position = self.set.index_of(entity)?;
        self.remove_at(entity, position, false)
    }

    fn put(&mut self, entity: Entity, component: T) -> Option<T> {
        let previous = self.set.insert_unhooked(entity, component);
        self.dirty.cover(self.set.len());
        self.added.cover(self.set.len());
        previous
    }

    fn remove_at(&mut self, entity: Entity, position: usize, tracked: bool) -> Option<T> {
        if tracked && !self.added.get(position) {
            self.removed.push(entity);
        }
        let last = self.set.len() - 1;
        self.dirty.swap_remove(position, last);
        self.added.swap_remove(position, last);
        let value = self.set.remove_unhooked(entity)?;
        self.set.hook_log().removed(entity, value)
    }

    pub(crate) fn end_tick(&mut self) {
        self.dirty.clear();
        self.added.clear();
        self.removed.clear();
    }
}

impl<T> Default for Tracked<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

/// A [`Tracked`] entry borrowed mutably. `DerefMut` marks it dirty, `Deref`
/// marks nothing.
pub struct TrackedMut<'a, T> {
    value: &'a mut T,
    dirty: Marker<'a>,
    position: usize,
}

impl<T> Deref for TrackedMut<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        self.value
    }
}

impl<T> DerefMut for TrackedMut<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        self.dirty.set(self.position);
        self.value
    }
}

/// [`Tracked`]'s mutable join term. Its items share the dirty bits.
pub struct TrackedJoin<'a, T: Component> {
    set: &'a mut SparseSet<T>,
    dirty: Marker<'a>,
}

// SAFETY: delegates to the `&mut SparseSet` term's guarantees: `entities`
// yields the dense entities once each, and `get` probes the set once per
// entity.
unsafe impl<'a, T: Component> Joinable<'a> for TrackedJoin<'a, T> {
    type Item = TrackedMut<'a, T>;

    #[inline]
    fn len(&self) -> usize {
        self.set.len()
    }

    #[inline]
    fn entities(&self) -> Entities<'a> {
        unsafe { self.set.entities_unbound() }
    }

    #[inline]
    unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
        let (position, value) = unsafe { self.set.get_mut_indexed_unbound(entity)? };
        Some(TrackedMut {
            value,
            dirty: self.dirty,
            position,
        })
    }
}

impl<T: Component> Table for Tracked<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    #[inline]
    fn len(&self) -> usize {
        self.len()
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.is_empty()
    }

    fn remove(&mut self, entity: Entity) -> bool {
        let Some(position) = self.set.index_of(entity) else {
            return false;
        };
        self.remove_at(entity, position, true);
        true
    }

    fn despawn_batch(&mut self, entities: &[Entity]) {
        for &entity in entities {
            self.remove_untracked(entity);
        }
    }

    #[inline]
    fn contains(&self, entity: Entity) -> bool {
        self.contains(entity)
    }

    fn clear(&mut self) {
        self.clear();
    }

    fn end_tick(&mut self) {
        self.end_tick();
    }
}

impl<T: Component> Storage for Tracked<T> {
    type Component = T;
    type Mut<'a> = TrackedMut<'a, T>;
    type JoinMut<'a> = TrackedJoin<'a, T>;

    #[inline]
    fn entities(&self) -> &[Entity] {
        self.entities()
    }

    #[inline]
    fn get(&self, entity: Entity) -> Option<&T> {
        self.get(entity)
    }

    #[inline]
    fn get_mut(&mut self, entity: Entity) -> Option<TrackedMut<'_, T>> {
        self.get_mut(entity)
    }

    fn insert(&mut self, entity: Entity, component: T) -> Option<T> {
        self.insert(entity, component)
    }

    fn remove(&mut self, entity: Entity) -> Option<T> {
        self.remove(entity)
    }

    #[inline]
    fn hook_log(&mut self) -> &mut HookLog<T> {
        self.hook_log()
    }

    #[inline]
    fn join_mut(&mut self) -> TrackedJoin<'_, T> {
        let (set, dirty) = self.split_mut();
        TrackedJoin { set, dirty }
    }
}

impl<T: Component> Tracking for Tracked<T> {
    #[inline]
    fn is_changed(&self, entity: Entity) -> bool {
        self.is_changed(entity)
    }

    #[inline]
    fn is_added(&self, entity: Entity) -> bool {
        self.is_added(entity)
    }

    fn removed(&self) -> &[Entity] {
        self.removed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::Component;
    use crate::ecs::query::{Changed, Mut, Query};
    use crate::ecs::storage::join::join;
    use crate::ecs::world::World;

    #[derive(Component, Debug, PartialEq)]
    #[component(storage = Tracked<Self>)]
    struct Hp(u32);

    #[derive(Component, Debug, PartialEq)]
    struct Regen(u32);

    fn changed(world: &World) -> Vec<Entity> {
        let mut changed = world.view::<Hp>().changed().collect::<Vec<_>>();
        changed.sort();
        changed
    }

    #[test]
    fn tracked_bits() {
        crate::ecs::init();
        let mut world = World::new();
        let entities = (0..4).map(|_| world.spawn()).collect::<Vec<_>>();
        for (index, &entity) in entities.iter().enumerate() {
            world.view_mut::<Hp>().insert(entity, Hp(index as u32));
        }
        assert!(world.view::<Hp>().is_added(entities[0]));
        assert_eq!(changed(&world), entities);
        world.end_tick();
        assert!(changed(&world).is_empty());
        assert!(!world.view::<Hp>().is_added(entities[0]));

        // reading through the mutable item marks nothing
        {
            let mut hps = world.view_mut::<Hp>();
            assert_eq!(hps.get_mut(entities[1]).unwrap().0, 1);
            hps.get_mut(entities[2]).unwrap().0 += 10;
            // a replaced value is dirty, not added
            assert_eq!(hps.insert(entities[3], Hp(30)), Some(Hp(3)));
        }
        assert_eq!(changed(&world), [entities[2], entities[3]]);
        assert!(!world.view::<Hp>().is_added(entities[3]));

        // the last entry's bits move into the hole
        world.view_mut::<Hp>().remove(entities[0]);
        let hps = world.view::<Hp>();
        assert_eq!(hps.entities(), [entities[3], entities[1], entities[2]]);
        assert!(hps.is_changed(entities[3]));
        assert!(!hps.is_changed(entities[1]));
        assert!(hps.is_changed(entities[2]));
    }

    #[test]
    fn tracked_remove_log() {
        crate::ecs::init();
        let mut world = World::new();
        let entities = (0..5).map(|_| world.spawn()).collect::<Vec<_>>();
        for &entity in &entities[..4] {
            world.view_mut::<Hp>().insert(entity, Hp(1));
        }
        world.end_tick();

        // an add then a remove cancel out
        world.view_mut::<Hp>().insert(entities[4], Hp(1));
        world.view_mut::<Hp>().remove(entities[4]);
        // a remove then an add is logged, and added
        world.view_mut::<Hp>().remove(entities[0]);
        world.view_mut::<Hp>().insert(entities[0], Hp(2));
        world.view_mut::<Hp>().remove(entities[0]);
        world.view_mut::<Hp>().insert(entities[0], Hp(3));
        // commands log as any removal, a despawn not at all
        world.commands().remove::<Hp>(entities[1]);
        world.commands().despawn(entities[2]);
        world.flush();

        let hps = world.view::<Hp>();
        assert_eq!(hps.removed(), [entities[0], entities[1]]);
        assert!(hps.is_added(entities[0]));
        assert_eq!(hps.get(entities[0]), Some(&Hp(3)));
        assert_eq!(Tracking::removed(&*hps), [entities[0], entities[1]]);
        drop(hps);

        world.end_tick();
        assert!(world.view::<Hp>().removed().is_empty());
    }

    #[test]
    fn tracked_query_changed() {
        crate::ecs::init();
        let mut world = World::new();
        let entities = (0..6).map(|_| world.spawn()).collect::<Vec<_>>();
        for (index, &entity) in entities.iter().enumerate() {
            world.view_mut::<Hp>().insert(entity, Hp(0));
            world
                .view_mut::<Regen>()
                .insert(entity, Regen(index as u32 % 2));
        }
        // not schema, so replicas stay writable
        world.entities().set_replica(entities[5], true);
        world.end_tick();

        let mut query = Query::<(Mut<Hp>, Regen)>::new(&world);
        for (_, (mut hp, regen)) in query.iter() {
            if regen.0 > 0 {
                hp.0 += regen.0;
            }
        }
        drop(query);
        assert_eq!(changed(&world), [entities[1], entities[3], entities[5]]);

        {
            let mut hps = world.view_mut::<Hp>();
            let regens = world.view::<Regen>();
            for (_, (mut hp, _)) in join((&mut hps, &regens)) {
                if hp.0 == 0 {
                    hp.0 = 5;
                    break;
                }
            }
        }

        let mut query = Query::<(Hp, Changed<Hp>)>::new(&world);
        let mut matched = query.iter().map(|(e, hp)| (e, hp.0)).collect::<Vec<_>>();
        matched.sort();
        assert_eq!(
            matched,
            [
                (entities[0], 5),
                (entities[1], 1),
                (entities[3], 1),
                (entities[5], 1)
            ]
        );
        drop(query);

        world.end_tick();
        assert_eq!(Query::<Changed<Hp>>::new(&world).iter().count(), 0);
    }
}
