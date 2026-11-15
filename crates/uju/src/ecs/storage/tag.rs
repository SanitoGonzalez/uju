use std::any::Any;

use crate::ecs::component::{Replicated, Replication};
use crate::ecs::entity::{Entity, Uid};
use crate::ecs::storage::{
    Storage, Tracking,
    section::{self, REMOVED, VALUES},
    table::Table,
    tracked::Tracked,
};
use crate::net::wire::{DecodeError, EncodeError};

/// The table of a tag: a component with no value, of which only the
/// additions and removals travel.
///
/// A tag is a unit struct, so its values take no memory, and the receiver
/// builds them with `Default`. Mutable access yields `&T`, since there is
/// nothing to write: a [`Mut`](crate::ecs::query::Mut) term reads, and like
/// any replicated table's skips replicas.
///
/// Inserting a tag the entity holds changes nothing and marks nothing, so
/// its changed entries are those added this tick.
pub struct TagTable<T: Replicated + Default>(Tracked<T>);

impl<T: Replicated + Default> TagTable<T> {
    pub fn new() -> Self {
        Self(Tracked::new())
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[inline]
    pub fn contains(&self, entity: Entity) -> bool {
        self.0.contains(entity)
    }

    #[inline]
    pub fn get(&self, entity: Entity) -> Option<&T> {
        self.0.get(entity)
    }

    #[inline]
    pub fn entities(&self) -> &[Entity] {
        self.0.entities()
    }

    pub fn iter(&self) -> impl Iterator<Item = (Entity, &T)> {
        self.0.iter()
    }

    /// Adds the tag to `entity`, returning the one it held. Only a new tag
    /// is marked added.
    pub fn insert(&mut self, entity: Entity, tag: T) -> Option<T> {
        if self.0.contains(entity) {
            return self.0.insert_untracked(entity, tag);
        }
        self.0.insert(entity, tag)
    }

    /// See [`Tracked::remove`].
    pub fn remove(&mut self, entity: Entity) -> Option<T> {
        self.0.remove(entity)
    }

    #[inline]
    pub fn is_added(&self, entity: Entity) -> bool {
        self.0.is_added(entity)
    }

    pub fn removed(&self) -> &[Entity] {
        self.0.removed()
    }

    /// The entities that gained the tag this tick, in dense order.
    pub fn added(&self) -> impl Iterator<Item = Entity> + '_ {
        self.0.changed()
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    fn write_section(
        &self,
        full: &[(Uid, Entity)],
        changed: &[(Uid, Entity)],
        removed: &[Uid],
        out: &mut Vec<u8>,
    ) -> Result<bool, EncodeError> {
        section::encode::<T>(out, |parts| {
            // a tag's value is its uid alone
            parts.part(VALUES, |out| {
                for &(uid, entity) in full.iter().chain(changed) {
                    if self.0.contains(entity) {
                        section::put_uid(out, uid);
                    }
                }
                Ok(())
            })?;
            parts.removed(removed)
        })
    }

    fn apply(
        &mut self,
        buf: &[u8],
        resolve: &mut dyn FnMut(Uid) -> Option<Entity>,
    ) -> Result<usize, DecodeError> {
        section::decode::<T>(buf, VALUES | REMOVED, |part, items, at| {
            section::read_uids::<T>(items, at, |uid| {
                let Some(entity) = resolve(uid) else {
                    return;
                };
                if part == VALUES {
                    self.0.insert_untracked(entity, T::default());
                } else {
                    self.0.remove_untracked(entity);
                }
            })
        })
    }
}

impl<T: Replicated + Default> Default for TagTable<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Replicated + Default> Table for TagTable<T> {
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
        self.remove(entity).is_some()
    }

    fn despawn_batch(&mut self, entities: &[Entity]) {
        Table::despawn_batch(&mut self.0, entities);
    }

    #[inline]
    fn contains(&self, entity: Entity) -> bool {
        self.contains(entity)
    }

    fn clear(&mut self) {
        self.clear();
    }

    fn end_tick(&mut self) {
        self.0.end_tick();
    }

    fn replication(&self) -> Option<Replication> {
        Some(Replication::of::<T>())
    }

    fn changed(&self, out: &mut Vec<Entity>) {
        out.extend(self.added());
    }

    fn removed(&self) -> &[Entity] {
        self.0.removed()
    }

    fn encode_section(
        &self,
        full: &[(Uid, Entity)],
        changed: &[(Uid, Entity)],
        removed: &[Uid],
        out: &mut Vec<u8>,
    ) -> Result<bool, EncodeError> {
        self.write_section(full, changed, removed, out)
    }

    fn apply_section(
        &mut self,
        buf: &[u8],
        resolve: &mut dyn FnMut(Uid) -> Option<Entity>,
    ) -> Result<usize, DecodeError> {
        self.apply(buf, resolve)
    }
}

impl<T: Replicated + Default> Storage for TagTable<T> {
    type Component = T;
    type Mut<'a> = &'a T;
    type JoinMut<'a> = &'a Tracked<T>;

    const REPLICATED: bool = true;

    #[inline]
    fn entities(&self) -> &[Entity] {
        self.entities()
    }

    #[inline]
    fn get(&self, entity: Entity) -> Option<&T> {
        self.get(entity)
    }

    #[inline]
    fn get_mut(&mut self, entity: Entity) -> Option<&T> {
        self.get(entity)
    }

    fn insert(&mut self, entity: Entity, component: T) -> Option<T> {
        self.insert(entity, component)
    }

    fn remove(&mut self, entity: Entity) -> Option<T> {
        self.remove(entity)
    }

    #[inline]
    fn join_mut(&mut self) -> &Tracked<T> {
        &self.0
    }
}

impl<T: Replicated + Default> Tracking for TagTable<T> {
    #[inline]
    fn is_changed(&self, entity: Entity) -> bool {
        self.is_added(entity)
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
    use crate::ecs::component::{Component as _, Visibility};
    use crate::ecs::query::{Changed, Mut, Query, With, Without};
    use crate::ecs::storage::section::testing::{apply, encode_changes, sim, uid};
    use crate::ecs::world::World;
    use crate::net::wire::DecodeErrorKind;

    /// A tag, as protoc would generate it from `tag Dead`.
    #[derive(Component, Debug, Clone, Copy, PartialEq, Default)]
    #[component(storage = TagTable<Self>)]
    struct Dead;

    impl Replicated for Dead {
        const WIRE_ID: u16 = 1;
        const VISIBILITY: Visibility = Visibility::Public;
    }

    #[derive(Component, Debug, PartialEq)]
    struct Body(u32);

    fn tagged(world: &World) -> Vec<Uid> {
        let mut query = Query::<With<Dead>>::new(world);
        let mut uids = query
            .iter()
            .map(|(e, ())| world.uid(e).unwrap())
            .collect::<Vec<_>>();
        uids.sort();
        uids
    }

    #[test]
    fn tag_section_layout() {
        crate::ecs::init();
        let world = World::new();
        let entity = world.spawn();
        world.view_mut::<Dead>().insert(entity, Dead);

        // the `Dead` section of the frames document's example
        let table = world.tables[Dead::id() as usize].borrow();
        let mut out = Vec::new();
        assert!(
            table
                .encode_section(&[], &[(uid(7), entity)], &[], &mut out)
                .unwrap()
        );
        #[rustfmt::skip]
        assert_eq!(out, [
            0x01, 0x00,
            0x01,
            0x08, 0x00, 0x00, 0x00,
            0x07, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
        ]);

        out.clear();
        assert!(table.encode_section(&[], &[], &[uid(3)], &mut out).unwrap());
        assert_eq!(out[2], 0b100);
        let gone = world.spawn();
        out.clear();
        assert!(
            !table
                .encode_section(&[(uid(8), gone)], &[], &[], &mut out)
                .unwrap()
        );
        assert!(out.is_empty());
    }

    #[test]
    fn tag_tracking() {
        crate::ecs::init();
        let mut world = World::new();
        let entities = (0..4).map(|_| world.spawn()).collect::<Vec<_>>();
        for &entity in &entities[..3] {
            world.view_mut::<Dead>().insert(entity, Dead);
            world.view_mut::<Body>().insert(entity, Body(0));
        }
        world.view_mut::<Body>().insert(entities[3], Body(0));
        assert_eq!(Query::<Changed<Dead>>::new(&world).iter().count(), 3);
        world.end_tick();

        // inserting a tag the entity holds changes nothing
        assert_eq!(
            world.view_mut::<Dead>().insert(entities[0], Dead),
            Some(Dead)
        );
        world.view_mut::<Dead>().insert(entities[3], Dead);
        world.view_mut::<Dead>().remove(entities[1]);
        let dead = world.view::<Dead>();
        assert_eq!(dead.added().collect::<Vec<_>>(), [entities[3]]);
        assert_eq!(dead.removed(), [entities[1]]);
        drop(dead);

        let mut query = Query::<(Body, Changed<Dead>)>::new(&world);
        assert_eq!(
            query.iter().map(|(e, _)| e).collect::<Vec<_>>(),
            [entities[3]]
        );
        drop(query);
        let mut query = Query::<(Body, Without<Dead>)>::new(&world);
        assert_eq!(
            query.iter().map(|(e, _)| e).collect::<Vec<_>>(),
            [entities[1]]
        );
        drop(query);

        // a `Mut` term reads, and skips replicas
        world.entities().set_replica(entities[2], true);
        let mut query = Query::<(Mut<Dead>, Body)>::new(&world);
        let owned = query
            .iter()
            .map(|(e, (dead, _))| (e, *dead))
            .collect::<Vec<_>>();
        assert_eq!(owned, [(entities[0], Dead), (entities[3], Dead)]);
    }

    #[test]
    fn tag_round_trip() {
        crate::ecs::init();
        let mut sim = sim();
        let edge = World::new();
        let entities = (0..4).map(|_| sim.spawn()).collect::<Vec<_>>();
        for &entity in &entities {
            sim.promote(entity);
        }
        let uids = entities
            .iter()
            .map(|&e| sim.uid(e).unwrap())
            .collect::<Vec<_>>();
        for &entity in &entities[..3] {
            sim.view_mut::<Dead>().insert(entity, Dead);
        }

        apply::<Dead>(&edge, &encode_changes::<Dead>(&sim, &[]));
        assert_eq!(tagged(&edge), uids[..3]);
        assert_eq!(edge.view::<Dead>().added().count(), 0);
        sim.end_tick();

        // a remove then an add is an add
        sim.view_mut::<Dead>().remove(entities[0]);
        sim.view_mut::<Dead>().insert(entities[0], Dead);
        sim.view_mut::<Dead>().remove(entities[1]);
        sim.view_mut::<Dead>().insert(entities[3], Dead);
        sim.view_mut::<Dead>().insert(entities[2], Dead);
        let section = encode_changes::<Dead>(&sim, &[]);
        #[rustfmt::skip]
        assert_eq!(section[..7], [0x01, 0x00, 0b101, 0x10, 0x00, 0x00, 0x00]);
        apply::<Dead>(&edge, &section);
        assert_eq!(tagged(&edge), [uids[0], uids[2], uids[3]]);
        assert!(edge.view::<Dead>().removed().is_empty());

        // a snapshot holds every tag
        let snapshot = World::new();
        let alive = sim.view::<Dead>().entities().to_vec();
        apply::<Dead>(&snapshot, &encode_changes::<Dead>(&sim, &alive));
        assert_eq!(tagged(&snapshot), tagged(&sim));

        // a tag has no changes part
        let mut table = edge.tables[Dead::id() as usize].borrow_mut();
        let error = table
            .apply_section(&[0b010, 0, 0, 0, 0], &mut |uid| edge.resolve(uid))
            .unwrap_err();
        assert_eq!(error.kind, DecodeErrorKind::Padding);
        let error = table
            .apply_section(&[0b001, 4, 0, 0, 0, 1, 0, 0, 0], &mut |uid| {
                edge.resolve(uid)
            })
            .unwrap_err();
        assert_eq!(error.kind, DecodeErrorKind::Length);
    }
}
