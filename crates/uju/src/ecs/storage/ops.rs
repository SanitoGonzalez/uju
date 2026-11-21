use std::any::{Any, type_name};
use std::cell::Cell;
use std::ops::Deref;

use crate::ecs::component::{Ops, Replication};
use crate::ecs::entity::{Entity, Uid};
use crate::ecs::hook::HookLog;
use crate::ecs::storage::{
    Storage, Tracking,
    bits::Marker,
    join::{Entities, Joinable},
    section::{self, CHANGES, REMOVED, VALUES},
    sparse_set::SparseSet,
    table::Table,
    tracked::Tracked,
};
use crate::net::wire::{Decode, DecodeError, Encode, EncodeError, de, en};

/// The end of a chain of ops, or the absence of one.
const NONE: u32 = u32::MAX;

/// The width of an ops vec's count: it has no `max_length`.
const COUNT: usize = 2;

/// The `len` before an entity's ops.
const LEN: usize = 2;

/// The arena positions of an entry's first and last op of the tick.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Span {
    first: u32,
    last: u32,
}

impl Span {
    /// No op recorded.
    const EMPTY: Self = Self {
        first: NONE,
        last: NONE,
    };

    /// The full encoding is sent, so ops are not recorded: the entry was
    /// added, inserted over or resynced this tick.
    const FULL: Self = Self {
        first: NONE,
        last: 0,
    };
}

/// The tick's ops of every entry, each with the arena position of its
/// entry's next op.
type Arena<O> = Vec<(O, u32)>;

/// The table of an `ops` component: a [`Tracked`] table whose values change
/// only through ops, with the tick's ops in one arena.
///
/// Mutable access yields an [`OpsMut`], which derefs to `&T` and applies
/// ops with [`OpsMut::apply`]. Each op applied is recorded and marks the
/// entry dirty. A dirty entry is sent as its ops, in order, and one added,
/// inserted over or [resynced](Self::resync) this tick as its full value.
///
/// Each entry keeps the arena positions of its first and last op, and each
/// op the position of the next, so the encoder reads an entity's ops in
/// order without sorting. The arena is cleared at the end of the tick.
///
/// Like a [`ReplaceTable`](crate::ecs::storage::replace::ReplaceTable), it
/// skips replicas on mutable access through a query or a view, and applies
/// received state untracked.
pub struct OpsTable<T: Ops> {
    tracked: Tracked<T>,
    /// One span per entry, in dense order.
    spans: Vec<Span>,
    arena: Arena<T::Op>,
}

impl<T: Ops> OpsTable<T> {
    pub fn new() -> Self {
        Self {
            tracked: Tracked::new(),
            spans: Vec::new(),
            arena: Vec::new(),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.tracked.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.tracked.is_empty()
    }

    #[inline]
    pub fn contains(&self, entity: Entity) -> bool {
        self.tracked.contains(entity)
    }

    #[inline]
    pub fn get(&self, entity: Entity) -> Option<&T> {
        self.tracked.get(entity)
    }

    /// `entity`'s value, in the wrapper that applies and records ops. It
    /// does not skip replicas: a
    /// [`ViewMut`](crate::ecs::storage::view::ViewMut)'s `get_mut` does.
    #[inline]
    pub fn get_mut(&mut self, entity: Entity) -> Option<OpsMut<'_, T>> {
        let position = self.tracked.index_of(entity)?;
        let (set, dirty) = self.tracked.split_mut();
        let spans = Cell::from_mut(self.spans.as_mut_slice()).as_slice_of_cells();
        Some(OpsMut {
            value: set.value_mut_at(position),
            span: &spans[position],
            arena: Cell::from_mut(&mut self.arena),
            dirty,
            position,
        })
    }

    /// `entity`'s value borrowed mutably, without recording anything.
    ///
    /// Replicas miss the change unless [`resync`](Self::resync) follows. It
    /// does not skip replicas either.
    #[inline]
    pub fn get_mut_untracked(&mut self, entity: Entity) -> Option<&mut T> {
        self.tracked.get_mut_untracked(entity)
    }

    /// Sends `entity`'s full value in the next delta, in place of its ops of
    /// this tick, which are dropped. Ops applied later in the tick are not
    /// recorded, since the value sent holds them. Returns whether `entity`
    /// holds a value.
    pub fn resync(&mut self, entity: Entity) -> bool {
        let Some(position) = self.tracked.index_of(entity) else {
            return false;
        };
        self.spans[position] = Span::FULL;
        self.tracked.split_mut().1.set(position);
        true
    }

    #[inline]
    pub fn entities(&self) -> &[Entity] {
        self.tracked.entities()
    }

    pub fn iter(&self) -> impl Iterator<Item = (Entity, &T)> {
        self.tracked.iter()
    }

    pub fn components(&self) -> impl Iterator<Item = &T> {
        self.tracked.components()
    }

    /// Inserts or replaces `entity`'s value, returning the previous one
    /// unless an `on_remove` hook keeps it. A replaced value is resynced, and a new one added. Either is sent
    /// whole.
    pub fn insert(&mut self, entity: Entity, component: T) -> Option<T> {
        let (replaced, previous) = self.tracked.insert_replacing(entity, component);
        match replaced {
            false => self.spans.push(Span::FULL),
            true => {
                let position = self.tracked.index_of(entity).unwrap();
                self.spans[position] = Span::FULL;
            }
        }
        previous
    }

    /// See [`Tracked::remove`]. The entry's ops of the tick are dropped.
    pub fn remove(&mut self, entity: Entity) -> Option<T> {
        self.remove_with(entity, Tracked::remove).flatten()
    }

    #[inline]
    pub fn is_changed(&self, entity: Entity) -> bool {
        self.tracked.is_changed(entity)
    }

    #[inline]
    pub fn is_added(&self, entity: Entity) -> bool {
        self.tracked.is_added(entity)
    }

    pub fn removed(&self) -> &[Entity] {
        self.tracked.removed()
    }

    pub fn changed(&self) -> impl Iterator<Item = Entity> + '_ {
        self.tracked.changed()
    }

    /// The ops recorded for `entity` this tick, in the order applied. None
    /// are recorded for an entry sent whole.
    pub fn ops(&self, entity: Entity) -> impl Iterator<Item = &T::Op> {
        let span = self
            .tracked
            .index_of(entity)
            .map_or(Span::EMPTY, |position| self.spans[position]);
        self.chain(span)
    }

    pub fn clear(&mut self) {
        self.tracked.clear();
        self.spans.clear();
        self.arena.clear();
    }

    fn chain(&self, span: Span) -> impl Iterator<Item = &T::Op> {
        let mut next = span.first;
        std::iter::from_fn(move || {
            let (op, after) = self.arena.get(next as usize)?;
            next = *after;
            Some(op)
        })
    }

    fn insert_untracked(&mut self, entity: Entity, component: T) {
        if self.tracked.insert_untracked(entity, component).is_none() {
            self.spans.push(Span::EMPTY);
        }
    }

    /// Removes `entity`'s value with `remove`, and moves the last entry's
    /// span into the hole, as the value moves. `None` when `entity` holds
    /// no value, else what `remove` returned.
    fn remove_with(
        &mut self,
        entity: Entity,
        remove: fn(&mut Tracked<T>, Entity) -> Option<T>,
    ) -> Option<Option<T>> {
        let position = self.tracked.index_of(entity)?;
        self.spans.swap_remove(position);
        Some(remove(&mut self.tracked, entity))
    }

    fn end_tick(&mut self) {
        // an entry has a span only where its dirty bit is set
        for position in self.tracked.changed_positions() {
            self.spans[position] = Span::EMPTY;
        }
        self.arena.clear();
        self.tracked.end_tick();
    }

    /// The count and the byte size of the ops vec of `span`, or `None` when
    /// the entry is sent whole: it was added or resynced, or its ops would
    /// not fit `len`.
    fn ops_size(&self, span: Span) -> Option<(usize, usize)> {
        if span == Span::FULL {
            return None;
        }
        let prefix = section::prefix_width(T::Op::MAX_ENCODED_SIZE);
        let (mut count, mut size) = (0, COUNT);
        for op in self.chain(span) {
            size += op.encoded_size() + if count > 0 { prefix } else { 0 };
            count += 1;
        }
        (size <= u16::MAX as usize).then_some((count, size))
    }

    /// Writes the ops vec of `span`, which holds `count` ops, to the start
    /// of `buf`.
    fn encode_ops(&self, span: Span, count: usize, buf: &mut [u8]) -> Result<usize, EncodeError> {
        let prefix = section::prefix_width(T::Op::MAX_ENCODED_SIZE);
        let mut w = en::Writer::new(buf, type_name::<T::Op>());
        w.len(COUNT, count)?;
        for (index, op) in self.chain(span).enumerate() {
            if index + 1 == count {
                op.write(&mut w)?;
            } else {
                let at = w.zeros(prefix)?;
                op.write(&mut w)?;
                let size = w.pos() - at - prefix;
                w.patch(at, prefix, size)?;
            }
        }
        Ok(w.pos())
    }

    fn write_section(
        &self,
        full: &[(Uid, Entity)],
        changed: &[(Uid, Entity)],
        removed: &[Uid],
        out: &mut Vec<u8>,
    ) -> Result<bool, EncodeError> {
        // an entry sent whole goes in the values part, one with ops that
        // fit `len` in the changes part
        let entries = |as_ops: bool| {
            changed.iter().filter_map(move |&(uid, entity)| {
                let position = self.tracked.index_of(entity)?;
                let span = self.spans[position];
                let size = self.ops_size(span);
                (size.is_some() == as_ops).then_some((uid, position, span, size))
            })
        };
        section::encode::<T>(out, |parts| {
            parts.part(VALUES, |out| {
                for &(uid, entity) in full {
                    if let Some(value) = self.get(entity) {
                        section::put_value(out, uid, value)?;
                    }
                }
                for (uid, position, ..) in entries(false) {
                    section::put_value(out, uid, self.tracked.value_at(position))?;
                }
                Ok(())
            })?;
            parts.part(CHANGES, |out| {
                for (uid, _, span, size) in entries(true) {
                    let (count, size) = size.unwrap();
                    section::put_sized(out, uid, LEN, size, |buf| {
                        self.encode_ops(span, count, buf)
                    })?;
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
        section::decode::<T>(
            buf,
            VALUES | CHANGES | REMOVED,
            |part, items, at| match part {
                VALUES => section::read_values::<T>(items, at, |uid, value| {
                    if let Some(entity) = resolve(uid) {
                        self.insert_untracked(entity, value);
                    }
                }),
                CHANGES => section::read_sized::<T>(items, at, None, LEN, |uid, bytes| {
                    // ops to a value the receiver does not hold are skipped
                    let mut value = resolve(uid).and_then(|e| self.tracked.get_mut_untracked(e));
                    let prefix = section::prefix_width(T::Op::MAX_ENCODED_SIZE);
                    de::prefixed(bytes, COUNT, prefix, 0, type_name::<T::Op>(), |op, _| {
                        let op = T::Op::decode(op)?;
                        if let Some(value) = value.as_deref_mut() {
                            value.apply(&op);
                        }
                        Ok(())
                    })?;
                    Ok(())
                }),
                _ => section::read_uids::<T>(items, at, |uid| {
                    if let Some(entity) = resolve(uid) {
                        self.remove_with(entity, Tracked::remove_untracked);
                    }
                }),
            },
        )
    }
}

impl<T: Ops> Default for OpsTable<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

/// An [`OpsTable`] entry borrowed mutably: it reads the value and applies
/// ops to it.
pub struct OpsMut<'a, T: Ops> {
    value: &'a mut T,
    span: &'a Cell<Span>,
    arena: &'a Cell<Arena<T::Op>>,
    dirty: Marker<'a>,
    position: usize,
}

impl<T: Ops> OpsMut<'_, T> {
    /// Applies `op` to the value with [`Ops::apply`], and records it for the
    /// tick's delta.
    pub fn apply(&mut self, op: T::Op) {
        self.value.apply(&op);
        self.dirty.set(self.position);
        let mut span = self.span.get();
        if span == Span::FULL {
            return;
        }
        // the wrappers alive at once share the arena, so it is taken out
        // of its cell to push
        let mut arena = self.arena.take();
        let at = u32::try_from(arena.len())
            .ok()
            .filter(|&at| at != NONE)
            .expect("more ops in a tick than a u32 counts");
        match span.last {
            NONE => span.first = at,
            last => arena[last as usize].1 = at,
        }
        span.last = at;
        arena.push((op, NONE));
        self.arena.set(arena);
        self.span.set(span);
    }
}

impl<T: Ops> Deref for OpsMut<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        self.value
    }
}

/// [`OpsTable`]'s mutable join term. Its items share the arena, the spans
/// and the dirty bits.
pub struct OpsJoin<'a, T: Ops> {
    set: &'a mut SparseSet<T>,
    dirty: Marker<'a>,
    spans: &'a [Cell<Span>],
    arena: &'a Cell<Arena<T::Op>>,
}

// SAFETY: delegates to the `&mut SparseSet` term's guarantees: `entities`
// yields the dense entities once each, and `get` probes the set once per
// entity.
unsafe impl<'a, T: Ops> Joinable<'a> for OpsJoin<'a, T> {
    type Item = OpsMut<'a, T>;

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
        Some(OpsMut {
            value,
            span: &self.spans[position],
            arena: self.arena,
            dirty: self.dirty,
            position,
        })
    }
}

impl<T: Ops> Table for OpsTable<T> {
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
        self.remove_with(entity, Tracked::remove).is_some()
    }

    fn despawn_batch(&mut self, entities: &[Entity]) {
        for &entity in entities {
            self.remove_with(entity, Tracked::remove_untracked);
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

    fn replication(&self) -> Option<Replication> {
        Some(Replication::of::<T>())
    }

    fn changed(&self, out: &mut Vec<Entity>) {
        out.extend(self.tracked.changed());
    }

    fn removed(&self) -> &[Entity] {
        self.tracked.removed()
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

impl<T: Ops> Storage for OpsTable<T> {
    type Component = T;
    type Mut<'a> = OpsMut<'a, T>;
    type JoinMut<'a> = OpsJoin<'a, T>;

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
    fn get_mut(&mut self, entity: Entity) -> Option<OpsMut<'_, T>> {
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
        self.tracked.hook_log()
    }

    #[inline]
    fn join_mut(&mut self) -> OpsJoin<'_, T> {
        let (set, dirty) = self.tracked.split_mut();
        OpsJoin {
            set,
            dirty,
            spans: Cell::from_mut(self.spans.as_mut_slice()).as_slice_of_cells(),
            arena: Cell::from_mut(&mut self.arena),
        }
    }
}

impl<T: Ops> Tracking for OpsTable<T> {
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
    use std::collections::BTreeMap;

    use super::*;
    use crate::ecs::Component;
    use crate::ecs::component::{Component as _, Replicated, Valued, Visibility};
    use crate::ecs::query::{Changed, Mut, Query};
    use crate::ecs::storage::join::join;
    use crate::ecs::storage::section::testing::{apply, encode_changes, sim, uid, values};
    use crate::ecs::world::World;
    use crate::net::wire::DecodeErrorKind;

    /// The wire format's inventory, written as protoc would generate it,
    /// with the `apply` the user writes:
    ///
    /// ```text
    /// union InventoryOp {
    ///     Add { item: u32, slot: u8 },
    ///     Remove(u8),
    ///     Clear,
    /// }
    ///
    /// @replicate(ops(InventoryOp))
    /// component Inventory {
    ///     items: map<u8, u32>,
    /// }
    /// ```
    #[derive(Component, Debug, Clone, PartialEq, Default)]
    #[component(storage = OpsTable<Self>)]
    struct Inventory {
        items: BTreeMap<u8, u32>,
    }

    impl Inventory {
        fn has_room(&self) -> bool {
            self.items.len() < 4
        }

        fn free_slot(&self) -> u8 {
            (0..).find(|slot| !self.items.contains_key(slot)).unwrap()
        }
    }

    impl Replicated for Inventory {
        const WIRE_ID: u16 = 7;
        const VISIBILITY: Visibility = Visibility::Owner;
    }

    impl Valued for Inventory {
        const SIZE: Option<usize> = None;
    }

    // the map's entries, packed: slot | item
    impl Encode for Inventory {
        const MAX_ENCODED_SIZE: Option<usize> = None;

        fn encoded_size(&self) -> usize {
            self.items.len() * 5
        }

        fn encode(&self, buf: &mut [u8]) -> Result<usize, EncodeError> {
            en::encode(self, buf, "Inventory")
        }

        fn write(&self, w: &mut en::Writer<'_>) -> Result<(), EncodeError> {
            for (&slot, &item) in &self.items {
                w.put(&[slot])?;
                w.put(&item.to_le_bytes())?;
            }
            Ok(())
        }
    }

    impl Decode<'_> for Inventory {
        fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
            de::packed(buf, 5, 0, "Inventory.items")?;
            let items = buf
                .chunks(5)
                .map(|entry| (entry[0], u32::from_le_bytes(entry[1..].try_into().unwrap())))
                .collect();
            Ok(Inventory { items })
        }
    }

    #[derive(Debug, Clone, PartialEq)]
    enum InventoryOp {
        Add { item: u32, slot: u8 },
        Remove(u8),
        Clear,
    }

    impl Encode for InventoryOp {
        const MAX_ENCODED_SIZE: Option<usize> = Some(6);

        fn encoded_size(&self) -> usize {
            match self {
                InventoryOp::Add { .. } => 6,
                InventoryOp::Remove(_) => 2,
                InventoryOp::Clear => 1,
            }
        }

        fn encode(&self, buf: &mut [u8]) -> Result<usize, EncodeError> {
            en::encode(self, buf, "InventoryOp")
        }

        fn write(&self, w: &mut en::Writer<'_>) -> Result<(), EncodeError> {
            match *self {
                InventoryOp::Add { item, slot } => {
                    w.put(&[1])?;
                    w.put(&item.to_le_bytes())?;
                    w.put(&[slot])
                }
                InventoryOp::Remove(slot) => w.put(&[2, slot]),
                InventoryOp::Clear => w.put(&[3]),
            }
        }
    }

    impl Decode<'_> for InventoryOp {
        fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
            de::at_least(buf, 1, 0, "InventoryOp")?;
            match buf[0] {
                1 => {
                    de::exact(buf, 6, 0, "InventoryOp")?;
                    Ok(InventoryOp::Add {
                        item: u32::from_le_bytes(buf[1..5].try_into().unwrap()),
                        slot: buf[5],
                    })
                }
                2 => {
                    de::exact(buf, 2, 0, "InventoryOp")?;
                    Ok(InventoryOp::Remove(buf[1]))
                }
                3 => {
                    de::exact(buf, 1, 0, "InventoryOp")?;
                    Ok(InventoryOp::Clear)
                }
                _ => de::tag(0, "InventoryOp"),
            }
        }
    }

    // total: an add replaces what the slot held, and removing an empty
    // slot leaves it empty
    impl Ops for Inventory {
        type Op = InventoryOp;

        fn apply(&mut self, op: &InventoryOp) {
            match *op {
                InventoryOp::Add { item, slot } => {
                    self.items.insert(slot, item);
                }
                InventoryOp::Remove(slot) => {
                    self.items.remove(&slot);
                }
                InventoryOp::Clear => self.items.clear(),
            }
        }
    }

    #[derive(Component, Debug)]
    struct Pickup(u32);

    use InventoryOp::{Add, Clear, Remove};

    fn inventory(items: &[(u8, u32)]) -> Inventory {
        Inventory {
            items: items.iter().copied().collect(),
        }
    }

    fn ops(world: &World, entity: Entity) -> Vec<InventoryOp> {
        world.view::<Inventory>().ops(entity).cloned().collect()
    }

    #[test]
    fn ops_layout() {
        crate::ecs::init();
        let mut world = sim();
        let entity = world.spawn();
        world.promote(entity);
        world
            .view_mut::<Inventory>()
            .insert(entity, inventory(&[(3, 9)]));
        world.end_tick();

        assert!(world.apply_op::<Inventory>(entity, Add { item: 7, slot: 2 }));
        assert!(world.apply_op::<Inventory>(entity, Remove(3)));
        assert_eq!(ops(&world, entity), [Add { item: 7, slot: 2 }, Remove(3)]);
        assert!(world.view::<Inventory>().is_changed(entity));

        // uid | len:u16 | a vec of the op union: count:u16, prefixed items
        let section = encode_changes::<Inventory>(&world, &[]);
        #[rustfmt::skip]
        assert_eq!(section, [
            0x07, 0x00,
            0x02,
            0x15, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
            0x0B, 0x00,
            0x02, 0x00,
            0x06, 0x01, 0x07, 0x00, 0x00, 0x00, 0x02,
            0x02, 0x03,
        ]);

        // an entering entity's full value goes in the values part
        let section = encode_changes::<Inventory>(&world, &[entity]);
        #[rustfmt::skip]
        assert_eq!(section, [
            0x07, 0x00,
            0x01,
            0x0F, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
            0x05, 0x00,
            0x02, 0x07, 0x00, 0x00, 0x00,
        ]);

        let table = world.tables[Inventory::id() as usize].borrow();
        let replication = table.replication().unwrap();
        assert_eq!(replication.wire_id, 7);
        assert_eq!(replication.visibility, Visibility::Owner);
    }

    #[test]
    fn ops_hooked_insert() {
        crate::ecs::init();
        let mut world = World::new();
        let removed = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        {
            let removed = removed.clone();
            world.on_remove::<Inventory>(
                move |_: &World, values: &mut Vec<(Entity, Inventory)>| {
                    removed.borrow_mut().extend(values.drain(..));
                },
            );
        }
        let (a, b) = (world.spawn(), world.spawn());
        world
            .view_mut::<Inventory>()
            .insert(a, Inventory::default());
        world
            .view_mut::<Inventory>()
            .insert(b, Inventory::default());
        world.end_tick();
        world.apply_op::<Inventory>(a, Add { item: 7, slot: 0 });

        // the hook keeps the replaced value, and the entry is sent whole
        let full = Inventory {
            items: [(1, 1)].into(),
        };
        assert_eq!(world.view_mut::<Inventory>().insert(a, full.clone()), None);
        assert!(ops(&world, a).is_empty());
        assert!(world.view::<Inventory>().is_changed(a));
        world.apply_op::<Inventory>(b, Clear);
        assert_eq!(world.view_mut::<Inventory>().remove(a), None);
        assert_eq!(ops(&world, b), [Clear]);
        world.flush();
        let added = Inventory {
            items: [(0, 7)].into(),
        };
        assert_eq!(*removed.borrow(), [(a, added), (a, full)]);
    }

    #[test]
    fn ops_arena() {
        crate::ecs::init();
        let mut world = World::new();
        let entities = (0..4).map(|_| world.spawn()).collect::<Vec<_>>();
        for &entity in &entities {
            world
                .view_mut::<Inventory>()
                .insert(entity, Inventory::default());
        }
        // ops after an add are not recorded: the value is sent whole
        world.apply_op::<Inventory>(entities[0], Clear);
        assert!(ops(&world, entities[0]).is_empty());
        assert!(world.view::<Inventory>().is_added(entities[0]));
        world.end_tick();

        // the ops of several entries interleave in the arena
        let [a, b, c, d] = entities[..] else {
            unreachable!()
        };
        for (entity, slot) in [(a, 0), (b, 0), (a, 1), (c, 0), (b, 1), (a, 2)] {
            let op = Add {
                item: slot as u32,
                slot,
            };
            assert!(world.apply_op::<Inventory>(entity, op));
        }
        let adds = |slots: &[u8]| {
            slots
                .iter()
                .map(|&slot| Add {
                    item: slot as u32,
                    slot,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(ops(&world, a), adds(&[0, 1, 2]));
        assert_eq!(ops(&world, b), adds(&[0, 1]));
        assert_eq!(ops(&world, c), adds(&[0]));
        assert!(ops(&world, d).is_empty());
        assert!(!world.view::<Inventory>().is_changed(d));

        // the last entry's span moves into the hole
        world
            .view_mut::<Inventory>()
            .insert(d, inventory(&[(1, 1)]));
        assert!(world.apply_op::<Inventory>(d, Clear));
        world.view_mut::<Inventory>().remove(b);
        let inventories = world.view::<Inventory>();
        assert_eq!(inventories.entities(), [a, d, c]);
        assert_eq!(inventories.ops(a).count(), 3);
        assert_eq!(inventories.ops(c).count(), 1);
        assert_eq!(inventories.ops(b).count(), 0);
        // insert over a value drops its ops and sends it whole
        assert_eq!(inventories.ops(d).count(), 0);
        assert!(inventories.is_changed(d));
        assert!(!inventories.is_added(d));
        assert_eq!(inventories.get(d), Some(&Inventory::default()));
        drop(inventories);

        // a removal resets the span: the entity comes back added
        world.view_mut::<Inventory>().remove(c);
        world
            .view_mut::<Inventory>()
            .insert(c, Inventory::default());
        assert!(world.apply_op::<Inventory>(c, Clear));
        assert!(ops(&world, c).is_empty());
        assert!(world.view::<Inventory>().is_added(c));

        // a resync drops the tick's ops, and records none after it
        assert!(world.resync::<Inventory>(a));
        assert!(ops(&world, a).is_empty());
        assert!(world.apply_op::<Inventory>(a, Remove(0)));
        assert!(ops(&world, a).is_empty());
        assert!(!world.resync::<Inventory>(b));

        world.end_tick();
        assert!(world.view::<Inventory>().arena.is_empty());
        assert_eq!(world.view::<Inventory>().changed().count(), 0);
        assert!(world.apply_op::<Inventory>(a, Remove(1)));
        assert!(world.apply_op::<Inventory>(c, Clear));
        assert_eq!(ops(&world, a), [Remove(1)]);
        assert_eq!(ops(&world, c), [Clear]);
    }

    #[test]
    fn ops_query() {
        crate::ecs::init();
        let mut world = World::new();
        let entities = (0..6).map(|_| world.spawn()).collect::<Vec<_>>();
        for (index, &entity) in entities.iter().enumerate() {
            let full = (0..4).map(|slot| (slot, 0)).collect::<Vec<_>>();
            let items = if index % 3 == 0 { &full[..] } else { &[] };
            world
                .view_mut::<Inventory>()
                .insert(entity, inventory(items));
            world
                .view_mut::<Pickup>()
                .insert(entity, Pickup(index as u32));
        }
        world.entities().set_replica(entities[5], true);
        world.end_tick();

        // the plan's loop: read the value, apply ops; replicas are skipped
        let mut query = Query::<(Mut<Inventory>, Pickup)>::new(&world);
        let mut seen = 0;
        for (_, (mut inventory, pickup)) in query.iter() {
            seen += 1;
            if inventory.has_room() {
                let slot = inventory.free_slot();
                let op = Add {
                    item: pickup.0,
                    slot,
                };
                inventory.apply(op);
            }
        }
        drop(query);
        assert_eq!(seen, 5);

        let mut query = Query::<(Inventory, Changed<Inventory>)>::new(&world);
        let mut changed = query.iter().map(|(e, _)| e).collect::<Vec<_>>();
        changed.sort();
        assert_eq!(changed, [entities[1], entities[2], entities[4]]);
        drop(query);
        let item = |e: Entity| {
            world
                .view::<Inventory>()
                .get(e)
                .unwrap()
                .items
                .get(&0)
                .copied()
        };
        assert_eq!(item(entities[2]), Some(2));
        assert_eq!(item(entities[5]), None);

        // a join's items share the arena
        {
            let mut inventories = world.view_mut::<Inventory>();
            assert!(inventories.get_mut(entities[5]).is_none());
            let pickups = world.view::<Pickup>();
            let mut items = join((&mut inventories, &pickups)).collect::<Vec<_>>();
            assert_eq!(items.len(), 5);
            for (_, (inventory, _)) in &mut items {
                inventory.apply(Clear);
            }
            for (_, (inventory, _)) in items.iter_mut().rev() {
                inventory.apply(Add { item: 1, slot: 9 });
            }
        }
        assert_eq!(
            ops(&world, entities[1]),
            [Add { item: 1, slot: 0 }, Clear, Add { item: 1, slot: 9 }]
        );
        assert_eq!(ops(&world, entities[0]), [Clear, Add { item: 1, slot: 9 }]);

        // replicas reject the shorthands
        assert!(!world.apply_op::<Inventory>(entities[5], Clear));
        assert!(!world.resync::<Inventory>(entities[5]));
        assert!(!world.view::<Inventory>().is_changed(entities[5]));
        let missing = world.spawn();
        assert!(!world.apply_op::<Inventory>(missing, Clear));
    }

    #[test]
    #[should_panic(expected = "already")]
    fn ops_apply_op_conflict_panics() {
        crate::ecs::init();
        let world = World::new();
        let entity = world.spawn();
        world
            .view_mut::<Inventory>()
            .insert(entity, Inventory::default());
        let mut query = Query::<Mut<Inventory>>::new(&world);
        for _ in query.iter() {
            world.apply_op::<Inventory>(entity, Clear);
        }
    }

    #[test]
    fn ops_round_trip() {
        crate::ecs::init();
        let mut sim = sim();
        let edge = World::new();
        let entities = (0..6).map(|_| sim.spawn()).collect::<Vec<_>>();
        for (index, &entity) in entities.iter().enumerate() {
            sim.promote(entity);
            sim.view_mut::<Inventory>()
                .insert(entity, inventory(&[(0, index as u32)]));
        }
        let uids = entities
            .iter()
            .map(|&e| sim.uid(e).unwrap())
            .collect::<Vec<_>>();

        // the first tick sends every added value
        let section = encode_changes::<Inventory>(&sim, &[]);
        assert_eq!(section[2], 0b001);
        apply::<Inventory>(&edge, &section);
        assert_eq!(values::<Inventory>(&edge), values::<Inventory>(&sim));
        sim.end_tick();

        let apply_op = |entity: Entity, op| assert!(sim.apply_op::<Inventory>(entity, op));
        apply_op(entities[0], Add { item: 5, slot: 1 });
        apply_op(entities[0], Remove(0));
        apply_op(entities[1], Clear);
        // an untracked change, then a resync
        sim.view_mut::<Inventory>()
            .get_mut_untracked(entities[2])
            .unwrap()
            .items
            .insert(4, 4);
        assert!(!sim.view::<Inventory>().is_changed(entities[2]));
        apply_op(entities[2], Add { item: 3, slot: 3 });
        sim.resync::<Inventory>(entities[2]);
        apply_op(entities[2], Remove(0));
        sim.view_mut::<Inventory>().remove(entities[3]);
        sim.view_mut::<Inventory>().remove(entities[4]);
        sim.view_mut::<Inventory>()
            .insert(entities[4], inventory(&[(8, 8)]));
        apply_op(entities[5], Add { item: 6, slot: 6 });
        sim.despawn(entities[5]);

        // ops in the changes part, the resynced and re-added values in the
        // values part
        let section = encode_changes::<Inventory>(&sim, &[]);
        assert_eq!(section[2], 0b111);
        apply::<Inventory>(&edge, &section);
        let mut expected = values::<Inventory>(&sim);
        // a despawn is the frame's `gone`, not a removal
        expected.push((uids[5], inventory(&[(0, 5)])));
        expected.sort_by_key(|(uid, _)| *uid);
        assert_eq!(values::<Inventory>(&edge), expected);

        // received state marks and records nothing on the receiver
        let edges = edge.view::<Inventory>();
        assert_eq!(edges.changed().count(), 0);
        assert!(edges.arena.is_empty());
        drop(edges);

        // a snapshot holds every value
        let snapshot = World::new();
        let alive = sim.view::<Inventory>().entities().to_vec();
        apply::<Inventory>(&snapshot, &encode_changes::<Inventory>(&sim, &alive));
        assert_eq!(values::<Inventory>(&snapshot), values::<Inventory>(&sim));
    }

    #[test]
    fn ops_overflow() {
        crate::ecs::init();
        let mut sim = sim();
        let edge = World::new();
        let (a, b) = (sim.spawn(), sim.spawn());
        for entity in [a, b] {
            sim.promote(entity);
            sim.view_mut::<Inventory>()
                .insert(entity, Inventory::default());
        }
        apply::<Inventory>(&edge, &encode_changes::<Inventory>(&sim, &[]));
        sim.end_tick();

        // an add takes 1 + 6 bytes, a remove 1 + 2, and the last op no
        // prefix: 6553 adds and 6552 removes take 65528 bytes with the count
        for index in 0..13105 {
            let op = match index % 2 {
                0 => Add { item: 1, slot: 0 },
                _ => Remove(0),
            };
            assert!(sim.apply_op::<Inventory>(a, op));
        }
        let section = encode_changes::<Inventory>(&sim, &[]);
        assert_eq!(section[2], 0b010);
        assert_eq!(&section[15..17], &65528u16.to_le_bytes());
        // 65531 bytes, then 65538
        for op in [Remove(0), Add { item: 2, slot: 1 }] {
            assert!(sim.apply_op::<Inventory>(a, op));
        }
        assert!(sim.apply_op::<Inventory>(b, Add { item: 3, slot: 0 }));

        // a's ops overflow, so its value is sent; b's ops still fit
        let section = encode_changes::<Inventory>(&sim, &[]);
        assert_eq!(section[2], 0b011);
        assert_eq!(&section[3..7], &(8 + 2 + 5u32).to_le_bytes());
        apply::<Inventory>(&edge, &section);
        assert_eq!(values::<Inventory>(&edge), values::<Inventory>(&sim));
        assert_eq!(sim.view::<Inventory>().get(a), Some(&inventory(&[(1, 2)])));
    }

    #[test]
    fn ops_apply_errors() {
        crate::ecs::init();
        let mut world = World::new();
        let entity = world.spawn();
        world
            .view_mut::<Inventory>()
            .insert(entity, Inventory::default());
        world.end_tick();
        let mut table = world.tables[Inventory::id() as usize].borrow_mut();
        let mut resolve = |uid| (uid == self::uid(1)).then_some(entity);
        let mut apply = |ops: &[u8]| {
            let mut buf = vec![0b010];
            buf.extend_from_slice(&(8 + 2 + ops.len() as u32).to_le_bytes());
            buf.extend_from_slice(&uid(1).to_bits().to_le_bytes());
            buf.extend_from_slice(&(ops.len() as u16).to_le_bytes());
            buf.extend_from_slice(ops);
            table.apply_section(&buf, &mut resolve).map(|_| ())
        };

        let kind = |result: Result<(), DecodeError>| result.unwrap_err().kind;
        // an undeclared tag, a truncated count, an op past its prefix
        assert_eq!(kind(apply(&[1, 0, 0])), DecodeErrorKind::Tag);
        assert_eq!(kind(apply(&[1])), DecodeErrorKind::Size);
        assert_eq!(kind(apply(&[2, 0, 9, 3])), DecodeErrorKind::Length);
        assert_eq!(kind(apply(&[1, 0, 2])), DecodeErrorKind::Size);
        assert_eq!(apply(&[2, 0, 1, 3, 1, 7, 0, 0, 0, 7]), Ok(()));
        assert_eq!(apply(&[0, 0]), Ok(()));
        drop(apply);
        drop(table);
        assert_eq!(
            world.view::<Inventory>().get(entity),
            Some(&inventory(&[(7, 7)]))
        );
        assert!(!world.view::<Inventory>().is_changed(entity));
    }
}
