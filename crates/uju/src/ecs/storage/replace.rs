use std::any::Any;
use std::fmt;

use crate::ecs::component::{Replace, Replication};
use crate::ecs::entity::{Entity, Uid};
use crate::ecs::hook::HookLog;
use crate::ecs::storage::{
    Storage, Tracking,
    section::{self, REMOVED, VALUES},
    table::{self, Table},
    tracked::{Tracked, TrackedJoin, TrackedMut},
};
use crate::net::wire::{DecodeError, EncodeError};

/// The table of a `replace` component: a [`Tracked`] table whose changes
/// travel as whole values.
///
/// Mutable access through a [`Mut`](crate::ecs::query::Mut) term or a
/// [`ViewMut`](crate::ecs::storage::view::ViewMut) skips replicas. State
/// received from the component's owner is applied by
/// [`Table::apply_section`], which marks and logs nothing.
pub struct ReplaceTable<T: Replace>(Tracked<T>);

impl<T: Replace> ReplaceTable<T> {
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

    /// See [`Tracked::get_mut`]. It does not skip replicas: a
    /// [`ViewMut`](crate::ecs::storage::view::ViewMut)'s `get_mut` does.
    #[inline]
    pub fn get_mut(&mut self, entity: Entity) -> Option<TrackedMut<'_, T>> {
        self.0.get_mut(entity)
    }

    #[inline]
    pub fn entities(&self) -> &[Entity] {
        self.0.entities()
    }

    pub fn iter(&self) -> impl Iterator<Item = (Entity, &T)> {
        self.0.iter()
    }

    pub fn components(&self) -> impl Iterator<Item = &T> {
        self.0.components()
    }

    /// See [`Tracked::insert`].
    pub fn insert(&mut self, entity: Entity, component: T) -> Option<T> {
        self.0.insert(entity, component)
    }

    /// See [`Tracked::remove`].
    pub fn remove(&mut self, entity: Entity) -> Option<T> {
        self.0.remove(entity)
    }

    #[inline]
    pub fn is_changed(&self, entity: Entity) -> bool {
        self.0.is_changed(entity)
    }

    #[inline]
    pub fn is_added(&self, entity: Entity) -> bool {
        self.0.is_added(entity)
    }

    pub fn removed(&self) -> &[Entity] {
        self.0.removed()
    }

    pub fn changed(&self) -> impl Iterator<Item = Entity> + '_ {
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
            // a replace change is a full value, so both lists are values
            parts.part(VALUES, |out| {
                for &(uid, entity) in full.iter().chain(changed) {
                    if let Some(value) = self.0.get(entity) {
                        section::put_value(out, uid, value)?;
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
            if part == VALUES {
                section::read_values::<T>(items, at, |uid, value| {
                    if let Some(entity) = resolve(uid) {
                        self.0.insert_untracked(entity, value);
                    }
                })
            } else {
                section::read_uids::<T>(items, at, |uid| {
                    if let Some(entity) = resolve(uid) {
                        self.0.remove_untracked(entity);
                    }
                })
            }
        })
    }
}

impl<T: Replace> Default for ReplaceTable<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Replace> Table for ReplaceTable<T> {
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
        Table::remove(&mut self.0, entity)
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

    fn inspect(&self, entity: Entity, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        table::inspect(self, entity, f)
    }

    fn end_tick(&mut self) {
        self.0.end_tick();
    }

    fn replication(&self) -> Option<Replication> {
        Some(Replication::of::<T>())
    }

    fn changed(&self, out: &mut Vec<Entity>) {
        out.extend(self.0.changed());
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

impl<T: Replace> Storage for ReplaceTable<T> {
    type Component = T;
    type Mut<'a> = TrackedMut<'a, T>;
    type JoinMut<'a> = TrackedJoin<'a, T>;

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
        self.0.hook_log()
    }

    #[inline]
    fn join_mut(&mut self) -> TrackedJoin<'_, T> {
        self.0.join_mut()
    }
}

impl<T: Replace> Tracking for ReplaceTable<T> {
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
    use std::num::NonZeroU32;

    use super::*;
    use crate::ecs::Component;
    use crate::ecs::cell::Cell;
    use crate::ecs::component::{Component as _, Replicated, Valued, Visibility};
    use crate::ecs::query::{Changed, Mut, Query, Replica};
    use crate::ecs::storage::join::join;
    use crate::ecs::storage::section::testing::{apply, encode_changes, sim, uid, values};
    use crate::ecs::world::World;
    use crate::net::wire::{Decode, DecodeErrorKind, Encode, en};

    #[derive(Component, Debug, Clone, PartialEq)]
    #[component(storage = ReplaceTable<Self>)]
    struct Health {
        hp: u32,
        max_hp: u32,
    }

    impl Replicated for Health {
        const WIRE_ID: u16 = 2;
        const VISIBILITY: Visibility = Visibility::Public;
    }

    impl Valued for Health {
        const SIZE: Option<usize> = Some(8);
    }

    impl Replace for Health {}

    impl Encode for Health {
        const MAX_ENCODED_SIZE: Option<usize> = Some(8);

        fn encoded_size(&self) -> usize {
            8
        }

        fn encode(&self, buf: &mut [u8]) -> Result<usize, EncodeError> {
            en::encode(self, buf, "Health")
        }

        fn write(&self, w: &mut en::Writer<'_>) -> Result<(), EncodeError> {
            w.put(&self.hp.to_le_bytes())?;
            w.put(&self.max_hp.to_le_bytes())
        }
    }

    impl Decode<'_> for Health {
        fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
            let buf: [u8; 8] = buf
                .try_into()
                .map_err(|_| DecodeError::new(0, "Health", DecodeErrorKind::Size))?;
            Ok(Health {
                hp: u32::from_le_bytes(buf[..4].try_into().unwrap()),
                max_hp: u32::from_le_bytes(buf[4..].try_into().unwrap()),
            })
        }
    }

    /// A variable-size component of a string, at most `$max` bytes.
    macro_rules! text {
        ($name:ident, $id:literal, $max:expr) => {
            #[derive(Component, Debug, Clone, PartialEq)]
            #[component(storage = ReplaceTable<Self>)]
            struct $name(String);

            impl Replicated for $name {
                const WIRE_ID: u16 = $id;
                const VISIBILITY: Visibility = Visibility::None;
            }

            impl Valued for $name {
                const SIZE: Option<usize> = None;
            }

            impl Replace for $name {}

            impl Encode for $name {
                const MAX_ENCODED_SIZE: Option<usize> = $max;

                fn encoded_size(&self) -> usize {
                    self.0.len()
                }

                fn encode(&self, buf: &mut [u8]) -> Result<usize, EncodeError> {
                    en::encode(self, buf, stringify!($name))
                }

                fn write(&self, w: &mut en::Writer<'_>) -> Result<(), EncodeError> {
                    w.put(self.0.as_bytes())
                }
            }

            impl Decode<'_> for $name {
                fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
                    let text = std::str::from_utf8(buf).map_err(|_| {
                        DecodeError::new(0, stringify!($name), DecodeErrorKind::Utf8)
                    })?;
                    Ok($name(text.to_owned()))
                }
            }
        };
    }

    // a 1-byte length prefix, and a 2-byte one
    text!(Label, 3, Some(64));
    text!(Note, 4, None);

    #[derive(Component, Debug, PartialEq)]
    struct Body(u32);

    #[test]
    fn replace_section_layout() {
        crate::ecs::init();
        let world = World::new();
        let (a, b) = (world.spawn(), world.spawn());
        let mut healths = world.view_mut::<Health>();
        healths.insert(
            a,
            Health {
                hp: 100,
                max_hp: 100,
            },
        );
        healths.insert(
            b,
            Health {
                hp: 80,
                max_hp: 100,
            },
        );
        drop(healths);

        // the `Health` section of the frames document's example
        let table = world.tables[Health::id() as usize].borrow();
        let mut out = vec![0xFF];
        let items = [(uid(5), a), (uid(6), b)];
        assert!(table.encode_section(&items, &[], &[], &mut out).unwrap());
        #[rustfmt::skip]
        assert_eq!(out, [
            0xFF,
            0x02, 0x00,
            0x01,
            0x20, 0x00, 0x00, 0x00,
            0x05, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
            0x64, 0x00, 0x00, 0x00, 0x64, 0x00, 0x00, 0x00,
            0x06, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
            0x50, 0x00, 0x00, 0x00, 0x64, 0x00, 0x00, 0x00,
        ]);

        // nothing to write writes nothing
        let gone = world.spawn();
        assert!(
            !table
                .encode_section(&[(uid(7), gone)], &[], &[], &mut out)
                .unwrap()
        );
        assert_eq!(out.len(), 40);

        let replication = table.replication().unwrap();
        assert_eq!(replication.wire_id, 2);
        assert_eq!(replication.visibility, Visibility::Public);
        assert!(replication.halo);
    }

    fn health(hp: u32) -> Health {
        Health { hp, max_hp: 100 }
    }

    #[test]
    fn replace_round_trip() {
        crate::ecs::init();
        let mut sim = sim();
        let edge = World::new();
        let entities = (0..5).map(|_| sim.spawn()).collect::<Vec<_>>();
        for (index, &entity) in entities.iter().enumerate() {
            sim.promote(entity);
            sim.view_mut::<Health>()
                .insert(entity, health(index as u32));
        }

        // the first tick sends every added value
        let section = encode_changes::<Health>(&sim, &[]);
        apply::<Health>(&edge, &section);
        assert_eq!(values::<Health>(&edge), values::<Health>(&sim));
        sim.end_tick();

        // received state marks nothing on the receiver
        let edges = edge.view::<Health>();
        assert_eq!(edges.changed().count(), 0);
        assert!(edges.removed().is_empty());
        drop(edges);

        let mut query = Query::<Mut<Health>>::new(&sim);
        for (_, mut health) in query.iter() {
            if health.hp % 2 == 0 {
                health.hp += 50;
            }
        }
        drop(query);
        sim.view_mut::<Health>().remove(entities[1]);
        sim.view_mut::<Health>().remove(entities[2]);
        sim.view_mut::<Health>().insert(entities[2], health(7));
        sim.despawn(entities[3]);

        let section = encode_changes::<Health>(&sim, &[]);
        apply::<Health>(&edge, &section);
        let edge_values = values::<Health>(&edge);
        let mut expected = values::<Health>(&sim);
        // a despawn is the frame's `gone`, not a removal
        expected.push((Uid::new(NonZeroU32::MIN, 3), health(3)));
        expected.sort_by_key(|(uid, _)| *uid);
        assert_eq!(edge_values, expected);

        // a snapshot holds every value
        let snapshot = World::new();
        let mut alive = sim.view::<Health>().entities().to_vec();
        alive.push(entities[3]);
        apply::<Health>(&snapshot, &encode_changes::<Health>(&sim, &alive));
        assert_eq!(values::<Health>(&snapshot), values::<Health>(&sim));
    }

    #[derive(Component, Debug, PartialEq)]
    struct Shape(u32);

    #[test]
    fn replace_hooks_on_replicas() {
        crate::ecs::init();
        let sim = sim();
        let mut edge = World::new();
        // a replica's health gets a shape of its own, outside the schema
        edge.on_insert::<Health>(|world: &World, added: &[Entity]| {
            for &entity in added {
                let hp = world.view::<Health>().get(entity).unwrap().hp;
                world.view_mut::<Shape>().insert(entity, Shape(hp));
            }
        });
        let freed = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        {
            let freed = freed.clone();
            edge.on_remove::<Health>(move |world: &World, values: &mut Vec<(Entity, Health)>| {
                for (entity, health) in values.drain(..) {
                    freed.borrow_mut().push(health.hp);
                    world.view_mut::<Shape>().remove(entity);
                }
            });
        }
        let entities = (0..3).map(|_| sim.spawn()).collect::<Vec<_>>();
        for (index, &entity) in entities.iter().enumerate() {
            sim.promote(entity);
            sim.view_mut::<Health>()
                .insert(entity, health(index as u32));
        }

        // entering replicas
        apply::<Health>(&edge, &encode_changes::<Health>(&sim, &[]));
        edge.flush();
        let shapes = |edge: &World| {
            let mut query = Query::<(Shape, Replica)>::new(edge);
            let mut shapes = query.iter().map(|(_, body)| body.0).collect::<Vec<_>>();
            shapes.sort();
            shapes
        };
        assert_eq!(shapes(&edge), [0, 1, 2]);

        // a changed value is a write, not an insert; a removal hands the
        // value over, and so does a replica's despawn
        let mut sim = sim;
        sim.end_tick();
        sim.view_mut::<Health>().get_mut(entities[0]).unwrap().hp = 10;
        sim.view_mut::<Health>().remove(entities[1]);
        apply::<Health>(&edge, &encode_changes::<Health>(&sim, &[]));
        let replica = edge.resolve(sim.uid(entities[2]).unwrap()).unwrap();
        edge.despawn(replica);
        edge.flush();
        assert_eq!(shapes(&edge), [0]);
        assert_eq!(*freed.borrow(), [1, 2]);
    }

    #[test]
    fn replace_variable_size() {
        crate::ecs::init();
        let sim = sim();
        let edge = World::new();
        let (a, b) = (sim.spawn(), sim.spawn());
        sim.promote(a);
        sim.promote(b);
        sim.view_mut::<Label>().insert(a, Label("a".repeat(64)));
        sim.view_mut::<Label>().insert(b, Label(String::new()));
        sim.view_mut::<Note>().insert(a, Note("é".repeat(300)));

        let labels = encode_changes::<Label>(&sim, &[]);
        // uid, 1-byte length, then the value
        assert_eq!(&labels[3..7], &(2 * (8 + 1) + 64u32).to_le_bytes());
        assert_eq!(labels[15], 64);
        apply::<Label>(&edge, &labels);
        let notes = encode_changes::<Note>(&sim, &[]);
        assert_eq!(&notes[15..17], &600u16.to_le_bytes());
        apply::<Note>(&edge, &notes);

        assert_eq!(values::<Label>(&edge), values::<Label>(&sim));
        assert_eq!(values::<Note>(&edge), values::<Note>(&sim));
    }

    #[test]
    fn replace_apply_errors() {
        crate::ecs::init();
        let world = World::new();
        let mut table = world.tables[Health::id() as usize].borrow_mut();
        let mut resolve = |uid| world.resolve(uid);
        let mut apply = |buf: &[u8]| table.apply_section(buf, &mut resolve).unwrap_err().kind;

        assert_eq!(apply(&[]), DecodeErrorKind::Size);
        // a `replace` component has no changes part
        assert_eq!(apply(&[0b010, 0, 0, 0, 0]), DecodeErrorKind::Padding);
        assert_eq!(apply(&[0b001, 4, 0, 0]), DecodeErrorKind::Size);
        assert_eq!(
            apply(&[0b100, 4, 0, 0, 0, 1, 0, 0, 0]),
            DecodeErrorKind::Length
        );
        let zero = [0b100, 8, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(apply(&zero), DecodeErrorKind::Range);
        let mut short = vec![0b001, 12, 0, 0, 0];
        short.extend_from_slice(&uid(1).to_bits().to_le_bytes());
        short.extend_from_slice(&[0; 4]);
        assert_eq!(apply(&short), DecodeErrorKind::Size);

        // an empty part reads, and an unknown uid is skipped
        assert_eq!(
            table.apply_section(&[0b001, 0, 0, 0, 0], &mut resolve),
            Ok(5)
        );
        let mut known = vec![0b101, 16, 0, 0, 0];
        known.extend_from_slice(&uid(1).to_bits().to_le_bytes());
        known.extend_from_slice(&[1, 0, 0, 0, 2, 0, 0, 0]);
        known.extend_from_slice(&[8, 0, 0, 0]);
        known.extend_from_slice(&uid(1).to_bits().to_le_bytes());
        known.extend_from_slice(&[0xAA; 3]);
        assert_eq!(
            table.apply_section(&known, &mut resolve),
            Ok(known.len() - 3)
        );
        assert!(table.is_empty());
    }

    #[test]
    fn replace_skips_replicas() {
        crate::ecs::init();
        let mut world = World::new();
        let entities = (0..4).map(|_| world.spawn()).collect::<Vec<_>>();
        for &entity in &entities {
            world.view_mut::<Health>().insert(entity, health(0));
            world
                .view_mut::<Label>()
                .insert(entity, Label(String::new()));
            world.view_mut::<Body>().insert(entity, Body(0));
        }
        world.entities().set_replica(entities[1], true);
        world.entities().set_replica(entities[2], true);
        world.end_tick();

        // two such terms filter once, and reads see both
        let mut query = Query::<(Mut<Health>, Mut<Label>, Body)>::new(&world);
        let owned = query.iter().map(|(e, _)| e).collect::<Vec<_>>();
        assert_eq!(owned, [entities[0], entities[3]]);
        drop(query);
        let mut query = Query::<(Mut<Health>, Replica)>::new(&world);
        assert_eq!(query.iter().count(), 0);
        drop(query);
        assert_eq!(Query::<Health>::new(&world).iter().count(), 4);

        // an optional write skips replicas as a whole, as `get` does
        let mut query = Query::<(Body, Option<Mut<Health>>)>::new(&world);
        let owned = query.iter().map(|(e, _)| e).collect::<Vec<_>>();
        assert_eq!(owned, [entities[0], entities[3]]);
        assert!(query.get(entities[1]).is_none());
        drop(query);

        // components outside the schema stay writable on replicas
        let mut query = Query::<(Mut<Body>, Health)>::new(&world);
        assert_eq!(query.iter().count(), 4);
        drop(query);

        {
            let mut healths = world.view_mut::<Health>();
            assert!(healths.get_mut(entities[1]).is_none());
            healths.get_mut(entities[0]).unwrap().hp = 1;
            let bodies = world.view::<Body>();
            for (_, (mut health, _)) in join((&mut healths, &bodies)) {
                health.hp += 1;
            }
        }
        let healths = world.view::<Health>();
        let hps = entities
            .iter()
            .map(|&e| healths.get(e).unwrap().hp)
            .collect::<Vec<_>>();
        assert_eq!(hps, [2, 0, 0, 1]);
        drop(healths);

        let mut query = Query::<(Health, Changed<Health>)>::new(&world);
        let changed = query.iter().map(|(e, _)| e).collect::<Vec<_>>();
        assert_eq!(changed, [entities[0], entities[3]]);
        drop(query);

        let cell = Cell::new(1);
        for &entity in &entities {
            world.entities().set_cell(entity, cell);
        }
        let mut query = Query::<Mut<Health>>::new(&world);
        assert_eq!(query.iter_in(cell).count(), 2);
    }
}
