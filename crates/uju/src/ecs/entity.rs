use std::cell::{Ref, RefCell, RefMut};
use std::collections::HashMap;
use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};

use crate::ecs::Event;
use crate::ecs::cell::{Cell, CellIndex};
use crate::ecs::hash::FoldBuildHasher;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(transparent)]
pub struct EntityIndex(u32);

impl EntityIndex {
    #[inline]
    pub const fn new(index: u32) -> Self {
        Self(index)
    }

    #[inline(always)]
    pub const fn to_bits(self) -> u32 {
        self.0
    }

    #[inline(always)]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }
}

impl fmt::Display for EntityIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(transparent)]
pub struct EntityGeneration(NonZeroU32);

impl EntityGeneration {
    #[inline]
    pub const fn new(generation: u32) -> Self {
        Self::from_bits(generation)
    }

    #[inline(always)]
    pub const fn to_bits(self) -> u32 {
        self.0.get()
    }

    #[inline(always)]
    pub const fn from_bits(bits: u32) -> Self {
        match NonZeroU32::new(bits) {
            Some(generation) => Self(generation),
            None => panic!("entity generation must be non-zero"),
        }
    }

    #[inline(always)]
    pub fn next(&self) -> Self {
        Self(self.0.checked_add(1).expect("entity generation overflow"))
    }
}

impl fmt::Display for EntityGeneration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Copy)]
#[repr(C, align(8))]
pub struct Entity {
    index: EntityIndex,
    generation: EntityGeneration,
}

impl PartialEq for Entity {
    #[inline]
    fn eq(&self, other: &Entity) -> bool {
        self.to_bits() == other.to_bits()
    }
}

impl Eq for Entity {}

impl PartialOrd for Entity {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Entity {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.to_bits().cmp(&other.to_bits())
    }
}

impl std::hash::Hash for Entity {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.to_bits().hash(state);
    }
}

impl Entity {
    pub const fn new(index: EntityIndex, generation: EntityGeneration) -> Self {
        Self { index, generation }
    }

    #[inline(always)]
    pub const fn to_bits(self) -> u64 {
        self.index.to_bits() as u64 | ((self.generation.to_bits() as u64) << 32)
    }

    #[inline(always)]
    pub const fn from_bits(bits: u64) -> Self {
        let raw_index = bits as u32;
        let raw_generation = (bits >> 32) as u32;
        Self {
            index: EntityIndex::from_bits(raw_index),
            generation: EntityGeneration::from_bits(raw_generation),
        }
    }

    #[inline]
    pub const fn index(self) -> EntityIndex {
        self.index
    }

    #[inline]
    pub const fn index_sparse(self) -> usize {
        self.index.0 as usize
    }

    #[inline]
    pub const fn generation(self) -> EntityGeneration {
        self.generation
    }
}

impl fmt::Debug for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}v{}", self.index, self.generation)
    }
}

pub struct Allocator {
    generations: Vec<EntityGeneration>,
    frees: Vec<EntityIndex>,
    alive: usize,
}

impl Allocator {
    pub fn new() -> Self {
        Self {
            generations: Vec::new(),
            frees: Vec::new(),
            alive: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.alive
    }

    pub fn is_empty(&self) -> bool {
        self.alive == 0
    }

    pub fn alloc(&mut self) -> Entity {
        self.try_alloc().expect("entity index space exhausted")
    }

    pub fn try_alloc(&mut self) -> Option<Entity> {
        let entity = match self.frees.pop() {
            Some(index) => {
                let generation = &mut self.generations[index.to_bits() as usize];
                *generation = generation.next(); // even -> odd: alive
                Entity::new(index, *generation)
            }
            None => {
                let index = EntityIndex::new(u32::try_from(self.generations.len()).ok()?);
                let generation = EntityGeneration::new(1);
                self.generations.push(generation);
                Entity::new(index, generation)
            }
        };
        self.alive += 1;
        Some(entity)
    }

    pub fn dealloc(&mut self, entity: Entity) -> bool {
        if !self.is_alive(entity) {
            return false;
        }

        let generation = &mut self.generations[entity.index().to_bits() as usize];
        *generation = generation.next(); // odd -> even: dead
        if generation.to_bits() != u32::MAX - 1 {
            self.frees.push(entity.index());
        } // else, retire the slot

        self.alive -= 1;
        true
    }

    pub fn is_alive(&self, entity: Entity) -> bool {
        self.generations
            .get(entity.index_sparse())
            .is_some_and(|&g| g == entity.generation() && g.to_bits() & 1 == 1)
    }

    /// The live entities, in index order.
    pub fn iter(&self) -> impl Iterator<Item = Entity> + '_ {
        self.generations
            .iter()
            .enumerate()
            .filter(|(_, generation)| generation.to_bits() & 1 == 1)
            .map(|(index, &generation)| Entity::new(EntityIndex::new(index as u32), generation))
    }
}

impl Default for Allocator {
    fn default() -> Self {
        Self::new()
    }
}

/// Cluster-wide identity of an entity, stable across migrations: `incarnation << 32 | serial`.
///
/// The incarnation names the shard instance that minted it, not its current owner. It is
/// never zero, so `Option<Uid>` stays 8 bytes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Uid(NonZeroU64);

impl Uid {
    #[inline]
    pub const fn new(incarnation: NonZeroU32, serial: u32) -> Self {
        Self::from_bits((incarnation.get() as u64) << 32 | serial as u64)
    }

    #[inline(always)]
    pub const fn to_bits(self) -> u64 {
        self.0.get()
    }

    #[inline(always)]
    pub const fn from_bits(bits: u64) -> Self {
        match NonZeroU64::new(bits) {
            Some(bits) if bits.get() >> 32 != 0 => Self(bits),
            _ => panic!("uid incarnation must be non-zero"),
        }
    }

    #[inline(always)]
    pub const fn incarnation(self) -> NonZeroU32 {
        match NonZeroU32::new((self.0.get() >> 32) as u32) {
            Some(incarnation) => incarnation,
            None => unreachable!(),
        }
    }

    #[inline(always)]
    pub const fn serial(self) -> u32 {
        self.0.get() as u32
    }
}

impl fmt::Debug for Uid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Uid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.incarnation(), self.serial())
    }
}

/// Mints [`Uid`]s for a shard from two incarnations: the current one, counting serials
/// from `0`, and a spare it switches to when the current one runs out.
///
/// It never asks for incarnations itself. Installed in a [`World`](crate::ecs::world::World),
/// each switch it makes while [promoting](crate::ecs::world::World::promote) shows as an
/// [`IncarnationSwitched`] event, and whoever installed it hands it the next spare with
/// [`set_spare`](Self::set_spare).
pub struct UidAllocator {
    current: NonZeroU32,
    next: u64,
    spare: Option<NonZeroU32>,
    retired: Vec<NonZeroU32>,
}

impl UidAllocator {
    /// Serials one incarnation can mint.
    pub const SERIALS: u64 = 1 << 32;

    pub const fn new(current: NonZeroU32, spare: NonZeroU32) -> Self {
        Self {
            current,
            next: 0,
            spare: Some(spare),
            retired: Vec::new(),
        }
    }

    /// The incarnation it mints from.
    pub const fn incarnation(&self) -> NonZeroU32 {
        self.current
    }

    /// The incarnation it switches to when the current one runs out, if any.
    pub const fn spare(&self) -> Option<NonZeroU32> {
        self.spare
    }

    /// Sets the spare, returning the previous one, if any.
    pub fn set_spare(&mut self, incarnation: NonZeroU32) -> Option<NonZeroU32> {
        self.spare.replace(incarnation)
    }

    /// Serials left in the current incarnation.
    pub const fn remaining(&self) -> u64 {
        Self::SERIALS - self.next
    }

    /// The incarnations it switched away from since the last call, oldest first.
    pub(crate) fn take_retired(&mut self) -> Vec<NonZeroU32> {
        std::mem::take(&mut self.retired)
    }

    /// Panics if the current incarnation has run out and there is no spare.
    pub fn alloc(&mut self) -> Uid {
        self.try_alloc()
            .expect("uid serials exhausted and no spare incarnation")
    }

    /// `None` if the current incarnation has run out and there is no spare.
    pub fn try_alloc(&mut self) -> Option<Uid> {
        if self.next == Self::SERIALS {
            let spare = self.spare.take()?;
            self.retired
                .push(std::mem::replace(&mut self.current, spare));
            self.next = 0;
        }
        let serial = self.next as u32;
        self.next += 1;
        Some(Uid::new(self.current, serial))
    }
}

/// The event a [`World`](crate::ecs::world::World) emits when its [`UidAllocator`]
/// switches to the spare: the spare is in use, and the allocator needs a new one.
#[derive(Event, Clone, Copy, PartialEq, Eq, Debug)]
pub struct IncarnationSwitched {
    /// The incarnation it switched away from, whose serials ran out.
    pub from: NonZeroU32,
}

/// Which entities are replicas: a bit per entity index, set by the runtime for
/// the entities another shard owns.
///
/// [`Owned`](crate::ecs::query::Owned) and [`Replica`](crate::ecs::query::Replica)
/// filter queries by it.
#[derive(Default)]
pub struct Replicas {
    words: Vec<u64>,
}

impl Replicas {
    /// Whether `entity`, which must be alive, is a replica.
    #[inline]
    pub fn contains(&self, entity: Entity) -> bool {
        let index = entity.index_sparse();
        self.words
            .get(index / 64)
            .is_some_and(|word| word & (1 << (index % 64)) != 0)
    }

    fn set(&mut self, index: usize, replica: bool) {
        let word = index / 64;
        if word >= self.words.len() {
            if !replica {
                return;
            }
            self.words.resize(word + 1, 0);
        }
        let mask = 1 << (index % 64);
        if replica {
            self.words[word] |= mask;
        } else {
            self.words[word] &= !mask;
        }
    }
}

/// An owned entity despawned this tick, with its uid and the cell it had at
/// the start of the tick.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Despawned {
    pub uid: Uid,
    pub cell: Cell,
}

/// What the [`Registry`] keeps per entity index.
#[derive(Clone, Copy, Default)]
struct Slot {
    uid: Option<Uid>,
    cell: Option<Cell>,
    /// The cell at the start of the tick.
    start: Option<Cell>,
}

/// The [`Registry`]'s entity allocator, with each entity's uid and cell.
pub(crate) struct Entries {
    local: Allocator,
    allocator: Option<UidAllocator>,
    slots: Vec<Slot>,
    entities: HashMap<Uid, Entity, FoldBuildHasher>,
    /// The indices whose cell changed this tick, possibly repeated.
    moved: Vec<usize>,
}

impl Entries {
    #[inline]
    pub(crate) fn is_alive(&self, entity: Entity) -> bool {
        self.local.is_alive(entity)
    }

    fn slot(&self, entity: Entity) -> Option<&Slot> {
        if !self.is_alive(entity) {
            return None;
        }
        self.slots.get(entity.index_sparse())
    }

    fn slot_mut(&mut self, index: usize) -> &mut Slot {
        if index >= self.slots.len() {
            self.slots.resize(index + 1, Slot::default());
        }
        &mut self.slots[index]
    }

    fn bind(&mut self, entity: Entity, uid: Uid) {
        self.slot_mut(entity.index_sparse()).uid = Some(uid);
        self.entities.insert(uid, entity);
    }
}

/// Owns a world's local entities, their [`Uid`]s and [`Cell`]s, which of them
/// are replicas, and the cell index.
///
/// A local [`Entity`] never leaves its world. A [`Uid`] is minted lazily, only once the
/// entity has to be named from outside (see
/// [`World::promote`](crate::ecs::world::World::promote)), or adopted
/// from another world (see [`adopt`](Self::adopt)).
///
/// The entity allocator, the replica flags and the cell index are each a borrow of
/// their own, so a query holding the flags or the index can still spawn.
pub struct Registry {
    entries: RefCell<Entries>,
    replicas: RefCell<Replicas>,
    cells: RefCell<CellIndex>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            entries: RefCell::new(Entries {
                local: Allocator::new(),
                allocator: None,
                slots: Vec::new(),
                entities: HashMap::default(),
                moved: Vec::new(),
            }),
            replicas: RefCell::default(),
            cells: RefCell::default(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.borrow().local.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.borrow().local.is_empty()
    }

    pub(crate) fn entries(&self) -> Ref<'_, Entries> {
        self.entries.borrow()
    }

    /// The allocator [`promote`](crate::ecs::world::World::promote) mints from, if one
    /// is installed.
    pub fn uid_allocator(&self) -> Option<Ref<'_, UidAllocator>> {
        Ref::filter_map(self.entries.borrow(), |entries| entries.allocator.as_ref()).ok()
    }

    /// The allocator [`promote`](crate::ecs::world::World::promote) mints from, if one
    /// is installed.
    pub fn uid_allocator_mut(&self) -> Option<RefMut<'_, UidAllocator>> {
        RefMut::filter_map(self.entries.borrow_mut(), |entries| {
            entries.allocator.as_mut()
        })
        .ok()
    }

    /// Installs the allocator [`promote`](crate::ecs::world::World::promote) mints from,
    /// returning the previous one.
    pub fn install_uid_allocator(&self, allocator: UidAllocator) -> Option<UidAllocator> {
        self.entries.borrow_mut().allocator.replace(allocator)
    }

    pub fn is_alive(&self, entity: Entity) -> bool {
        self.entries.borrow().is_alive(entity)
    }

    /// Replaces the contents of `out` with the live entities, in index
    /// order. The allocator is borrowed only during the call, so a loop over
    /// `out` can spawn.
    pub(crate) fn alive_into(&self, out: &mut Vec<Entity>) {
        out.clear();
        out.extend(self.entries.borrow().local.iter());
    }

    /// Allocates a local entity. It has no [`Uid`] until promoted.
    pub fn spawn(&self) -> Entity {
        self.entries.borrow_mut().local.alloc()
    }

    /// Frees every live entity of `entities`, forgets their uids and cells and
    /// clears their replica flags. Logs each owned one that had a uid and a cell
    /// at the start of the tick into `log`. Returns how many it freed.
    pub(crate) fn despawn_batch(&self, entities: &[Entity], log: &mut Vec<Despawned>) -> usize {
        let mut entries = self.entries.borrow_mut();
        let mut replicas = self.replicas.borrow_mut();
        let mut cells = self.cells.borrow_mut();

        let mut despawned = 0;
        for &entity in entities {
            if !entries.local.dealloc(entity) {
                continue;
            }
            despawned += 1;

            // the slot will be recycled, so nothing of it may outlive the entity
            let index = entity.index_sparse();
            let replica = replicas.contains(entity);
            replicas.set(index, false);
            let Some(slot) = entries.slots.get_mut(index).map(std::mem::take) else {
                continue;
            };
            if let Some(uid) = slot.uid {
                entries.entities.remove(&uid);
            }
            if let Some(cell) = slot.cell {
                cells.remove(cell, entity);
            }
            if !replica && let (Some(uid), Some(cell)) = (slot.uid, slot.start) {
                log.push(Despawned { uid, cell });
            }
        }
        despawned
    }

    /// The [`Uid`] of `entity`, if it is alive and has one.
    pub fn uid(&self, entity: Entity) -> Option<Uid> {
        self.entries.borrow().slot(entity)?.uid
    }

    /// The [`Uid`] of `entity`, minting one on first use. `None` if `entity` is dead, or
    /// if it has no uid and no allocator is installed.
    ///
    /// Panics if the installed allocator's current incarnation has run out and it has no
    /// spare (see [`UidAllocator::alloc`]). A switch to the spare is left in the
    /// allocator for [`World::promote`](crate::ecs::world::World::promote) to emit.
    pub(crate) fn promote(&self, entity: Entity) -> Option<Uid> {
        let mut entries = self.entries.borrow_mut();
        if !entries.is_alive(entity) {
            return None;
        }
        if let Some(uid) = entries.slot(entity).and_then(|slot| slot.uid) {
            return Some(uid);
        }

        let uid = entries.allocator.as_mut()?.alloc();
        entries.bind(entity, uid);
        Some(uid)
    }

    /// The local entity carrying `uid`, if this world holds it.
    pub fn resolve(&self, uid: Uid) -> Option<Entity> {
        self.entries.borrow().entities.get(&uid).copied()
    }

    /// Spawns a local entity for an existing `uid`, e.g. one migrating in or replicated
    /// from another world. `None` if `uid` already has a local entity here.
    pub fn adopt(&self, uid: Uid) -> Option<Entity> {
        let mut entries = self.entries.borrow_mut();
        if entries.entities.contains_key(&uid) {
            return None;
        }

        let entity = entries.local.alloc();
        entries.bind(entity, uid);
        Some(entity)
    }

    /// The [`Cell`] of `entity`, if it is alive and has one.
    pub fn cell(&self, entity: Entity) -> Option<Cell> {
        self.entries.borrow().slot(entity)?.cell
    }

    /// The [`Cell`] `entity` had at the start of the tick, if it is alive and
    /// had one.
    #[cfg_attr(not(test), expect(dead_code, reason = "read by the encoder"))]
    pub(crate) fn tick_start_cell(&self, entity: Entity) -> Option<Cell> {
        self.entries.borrow().slot(entity)?.start
    }

    /// Moves `entity` to `cell` and updates the cell index. Does nothing if
    /// `entity` is dead.
    ///
    /// Panics if the cell index is borrowed, such as by a query's `iter_in`.
    pub(crate) fn set_cell(&self, entity: Entity, cell: Cell) {
        let mut entries = self.entries.borrow_mut();
        if !entries.is_alive(entity) {
            return;
        }

        let index = entity.index_sparse();
        let slot = entries.slot_mut(index);
        let previous = slot.cell;
        if previous == Some(cell) {
            return;
        }
        let first_move = previous == slot.start;
        slot.cell = Some(cell);
        if first_move {
            entries.moved.push(index);
        }

        let mut cells = self.cells.borrow_mut();
        if let Some(previous) = previous {
            cells.remove(previous, entity);
        }
        cells.insert(cell, entity);
    }

    /// The cell index: per [`Cell`], the entities in it.
    pub fn cell_index(&self) -> Ref<'_, CellIndex> {
        self.cells.borrow()
    }

    /// Whether `entity` is alive and a replica of an entity another shard owns.
    pub fn is_replica(&self, entity: Entity) -> bool {
        self.is_alive(entity) && self.replicas.borrow().contains(entity)
    }

    /// The replica flags.
    pub fn replicas(&self) -> Ref<'_, Replicas> {
        self.replicas.borrow()
    }

    /// Marks `entity` as a replica, or as owned. Does nothing if `entity` is
    /// dead.
    #[cfg_attr(not(test), expect(dead_code, reason = "set by the runtime"))]
    pub(crate) fn set_replica(&self, entity: Entity, replica: bool) {
        if self.is_alive(entity) {
            self.replicas
                .borrow_mut()
                .set(entity.index_sparse(), replica);
        }
    }

    /// Rolls each entity's cell into its cell at the start of the tick.
    pub(crate) fn end_tick(&mut self) {
        let entries = self.entries.get_mut();
        for index in entries.moved.drain(..) {
            if let Some(slot) = entries.slots.get_mut(index) {
                slot.start = slot.cell;
            }
        }
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::world::World;

    #[test]
    fn entity_option_niche() {
        // the non-zero generation lets `None` hide in the zero bits
        assert_eq!(size_of::<Option<Entity>>(), size_of::<Entity>());
    }

    #[test]
    #[should_panic(expected = "entity generation must be non-zero")]
    fn entity_generation_zero() {
        _ = Entity::from_bits(0);
    }

    #[test]
    fn allocator_alloc() {
        let mut allocator = Allocator::new();
        let e0 = allocator.alloc();
        let e1 = allocator.alloc();

        assert_eq!(e0.index(), EntityIndex::from_bits(0));
        assert_eq!(e1.index(), EntityIndex::from_bits(1));
        assert_eq!(e0.generation(), EntityGeneration::from_bits(1));
        assert_eq!(e1.generation(), EntityGeneration::from_bits(1));

        assert!(allocator.is_alive(e0));
        assert!(allocator.is_alive(e1));
        assert_eq!(allocator.alive, 2);
        assert_eq!(allocator.len(), 2);
    }

    #[test]
    fn allocator_try_alloc() {
        let mut allocator = Allocator::new();

        let e0 = allocator.try_alloc().expect("fresh slot");
        assert_eq!(e0.index(), EntityIndex::from_bits(0));
        assert_eq!(e0.generation(), EntityGeneration::from_bits(1));
        assert!(allocator.is_alive(e0));
        assert_eq!(allocator.len(), 1);

        assert!(allocator.dealloc(e0));
        let recycled = allocator.try_alloc().expect("recycled slot");
        assert_eq!(recycled.index(), e0.index());
        assert_eq!(allocator.len(), 1);
    }

    #[test]
    fn allocator_dealloc() {
        let mut allocator = Allocator::new();
        let e0 = allocator.alloc();

        assert!(allocator.dealloc(e0));
        assert!(!allocator.is_alive(e0));
        assert_eq!(allocator.alive, 0);

        // dealloc is idempotent, and must not push the index twice
        assert!(!allocator.dealloc(e0));
        assert_eq!(allocator.frees.len(), 1);
        assert_eq!(allocator.alive, 0);
    }

    #[test]
    fn allocator_recycle() {
        let mut allocator = Allocator::new();
        let e0 = allocator.alloc();
        let e1 = allocator.alloc();
        assert!(allocator.dealloc(e0));

        let recycled = allocator.alloc();
        assert_eq!(recycled.index(), e0.index());
        assert_eq!(recycled.generation(), EntityGeneration::from_bits(3)); // 1 -> 2 dead -> 3
        assert_ne!(recycled, e0);
        assert_eq!(allocator.generations.len(), 2); // slot reused, not grown

        assert!(allocator.is_alive(recycled));
        assert!(allocator.is_alive(e1));
        assert!(!allocator.is_alive(e0)); // the stale handle stays dead
        assert!(!allocator.dealloc(e0));
        assert!(allocator.is_alive(recycled));
    }

    #[test]
    fn allocator_recycle_order() {
        let mut allocator = Allocator::new();
        let entities = [allocator.alloc(), allocator.alloc(), allocator.alloc()];

        assert!(allocator.dealloc(entities[0]));
        assert!(allocator.dealloc(entities[2]));

        // last freed comes back first
        assert_eq!(allocator.alloc().index(), entities[2].index());
        assert_eq!(allocator.alloc().index(), entities[0].index());
        // free list drained, so allocation grows again
        assert_eq!(allocator.alloc().index(), EntityIndex::from_bits(3));
        assert_eq!(allocator.alive, 4);
    }

    #[test]
    fn allocator_unknown_entity() {
        let mut allocator = Allocator::new();
        let unknown = Entity::new(EntityIndex::from_bits(7), EntityGeneration::from_bits(1));

        assert!(!allocator.is_alive(unknown));
        assert!(!allocator.dealloc(unknown));

        let e0 = allocator.alloc();
        let forged = Entity::new(e0.index(), EntityGeneration::from_bits(3));

        assert!(!allocator.is_alive(forged));
        assert!(!allocator.dealloc(forged));
        assert!(allocator.is_alive(e0));
    }

    #[test]
    fn allocator_retire_exhausted_slot() {
        let mut allocator = Allocator::new();
        let entity = allocator.alloc();

        // fast-forward the slot to its last usable alive generation
        let last_generation = EntityGeneration::from_bits(u32::MAX - 2);
        allocator.generations[entity.index_sparse()] = last_generation;
        let last = Entity::new(entity.index(), last_generation);
        assert!(allocator.is_alive(last));

        assert!(allocator.dealloc(last));
        assert!(allocator.frees.is_empty()); // retired instead of recycled
        assert_eq!(allocator.alloc().index(), EntityIndex::from_bits(1));
    }

    fn incarnation(n: u32) -> NonZeroU32 {
        NonZeroU32::new(n).unwrap()
    }

    fn uid(inc: u32, serial: u32) -> Uid {
        Uid::new(incarnation(inc), serial)
    }

    #[test]
    fn uid_layout() {
        let u = uid(3, 7);
        assert_eq!(u.to_bits(), 3 << 32 | 7);
        assert_eq!(u.incarnation(), incarnation(3));
        assert_eq!(u.serial(), 7);
        assert_eq!(Uid::from_bits(u.to_bits()), u);
        assert_eq!(size_of::<Uid>(), 8);
        assert_eq!(size_of::<Option<Uid>>(), 8);
    }

    #[test]
    #[should_panic(expected = "uid incarnation must be non-zero")]
    fn uid_incarnation_zero() {
        _ = Uid::from_bits(7);
    }

    fn allocator() -> UidAllocator {
        UidAllocator::new(incarnation(3), incarnation(4))
    }

    #[test]
    fn uid_allocator_alloc() {
        let mut allocator = allocator();
        assert_eq!(allocator.incarnation(), incarnation(3));
        assert_eq!(allocator.spare(), Some(incarnation(4)));
        assert_eq!(allocator.remaining(), UidAllocator::SERIALS);

        assert_eq!(allocator.alloc(), uid(3, 0));
        assert_eq!(allocator.alloc(), uid(3, 1));
        assert_eq!(allocator.remaining(), UidAllocator::SERIALS - 2);
        assert!(allocator.take_retired().is_empty());
    }

    #[test]
    fn uid_allocator_switch() {
        let mut allocator = allocator();
        allocator.next = UidAllocator::SERIALS - 1;

        assert_eq!(allocator.alloc(), uid(3, u32::MAX));
        assert_eq!(allocator.remaining(), 0);
        assert!(allocator.take_retired().is_empty()); // switches on demand only

        assert_eq!(allocator.alloc(), uid(4, 0));
        assert_eq!(allocator.incarnation(), incarnation(4));
        assert_eq!(allocator.spare(), None);
        assert_eq!(allocator.remaining(), UidAllocator::SERIALS - 1);
        assert_eq!(allocator.take_retired(), [incarnation(3)]);
        assert!(allocator.take_retired().is_empty());
    }

    #[test]
    fn uid_allocator_set_spare() {
        let mut allocator = allocator();
        allocator.next = UidAllocator::SERIALS;
        allocator.alloc();

        assert_eq!(allocator.set_spare(incarnation(5)), None);
        allocator.next = UidAllocator::SERIALS;
        assert_eq!(allocator.alloc(), uid(5, 0));
        // retired until taken, oldest first
        assert_eq!(allocator.take_retired(), [incarnation(3), incarnation(4)]);

        // a spare not yet used is replaced
        assert_eq!(allocator.set_spare(incarnation(6)), None);
        assert_eq!(allocator.set_spare(incarnation(7)), Some(incarnation(6)));
    }

    #[test]
    fn uid_allocator_exhausted() {
        let mut allocator = allocator();
        allocator.next = UidAllocator::SERIALS;
        assert_eq!(allocator.try_alloc(), Some(uid(4, 0)));

        allocator.next = UidAllocator::SERIALS;
        assert_eq!(allocator.try_alloc(), None);
        assert_eq!(allocator.try_alloc(), None);
        assert_eq!(allocator.incarnation(), incarnation(4));
        assert_eq!(allocator.take_retired(), [incarnation(3)]);
    }

    #[test]
    #[should_panic(expected = "uid serials exhausted and no spare incarnation")]
    fn uid_allocator_exhausted_panics() {
        let mut allocator = allocator();
        allocator.next = UidAllocator::SERIALS;
        allocator.alloc();
        allocator.next = UidAllocator::SERIALS;
        allocator.alloc();
    }

    #[test]
    fn uid_allocator_distinct_incarnations() {
        // incarnations count independently, so equal serials are the normal case
        let a = UidAllocator::new(incarnation(3), incarnation(4)).alloc();
        let b = UidAllocator::new(incarnation(5), incarnation(6)).alloc();

        assert_eq!(a.serial(), b.serial());
        assert_ne!(a, b);
    }

    fn registry() -> Registry {
        let registry = Registry::new();
        registry.install_uid_allocator(allocator());
        registry
    }

    impl Registry {
        fn despawn(&self, entity: Entity) -> bool {
            self.despawn_batch(&[entity], &mut Vec::new()) == 1
        }
    }

    #[test]
    fn registry_spawn_is_local_only() {
        let registry = registry();
        let e0 = registry.spawn();

        assert!(registry.is_alive(e0));
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.uid(e0), None);
        // nothing minted yet
        assert_eq!(
            registry.uid_allocator().unwrap().remaining(),
            UidAllocator::SERIALS
        );
    }

    #[test]
    fn registry_promote() {
        let registry = registry();
        let e0 = registry.spawn();
        let e1 = registry.spawn();

        let u1 = registry.promote(e1).unwrap();
        assert_eq!(u1, uid(3, 0));
        assert_eq!(registry.promote(e1), Some(u1)); // idempotent, no re-mint
        assert_eq!(
            registry.uid_allocator().unwrap().remaining(),
            UidAllocator::SERIALS - 1
        );

        assert_eq!(registry.uid(e1), Some(u1));
        assert_eq!(registry.resolve(u1), Some(e1));
        assert_eq!(registry.uid(e0), None);
    }

    #[test]
    fn registry_promote_dead() {
        let registry = registry();
        let e0 = registry.spawn();
        assert!(registry.despawn(e0));

        assert_eq!(registry.promote(e0), None);
        assert_eq!(
            registry.uid_allocator().unwrap().remaining(),
            UidAllocator::SERIALS
        );
    }

    #[test]
    fn registry_promote_without_allocator() {
        let registry = Registry::new();
        let e0 = registry.spawn();
        assert_eq!(registry.promote(e0), None);
        assert_eq!(registry.uid(e0), None);

        // an adopted uid is still reported
        let e1 = registry.adopt(uid(9, 42)).unwrap();
        assert_eq!(registry.promote(e1), Some(uid(9, 42)));
    }

    #[test]
    fn registry_promote_switches() {
        let registry = registry();
        let e0 = registry.spawn();
        let u0 = registry.promote(e0).unwrap();

        registry.uid_allocator_mut().unwrap().next = UidAllocator::SERIALS;
        let e1 = registry.spawn();
        assert_eq!(registry.promote(e1), Some(uid(4, 0)));
        assert_eq!(registry.promote(e0), Some(u0)); // minted ids stay

        let mut allocator = registry.uid_allocator_mut().unwrap();
        assert_eq!(allocator.take_retired(), [incarnation(3)]);
        assert_eq!(allocator.set_spare(incarnation(5)), None);
    }

    #[test]
    fn world_promote_emits_switches() {
        crate::ecs::init();
        let mut world = World::new();
        world.install_uid_allocator(allocator());
        let e0 = world.spawn();
        assert_eq!(world.promote(e0), Some(uid(3, 0)));
        assert!(world.events::<IncarnationSwitched>().is_empty());

        // outside any schedule, as while applying a migration
        world.uid_allocator_mut().unwrap().next = UidAllocator::SERIALS;
        let e1 = world.spawn();
        assert_eq!(world.promote(e1), Some(uid(4, 0)));
        assert_eq!(world.promote(e1), Some(uid(4, 0)));
        let switched = IncarnationSwitched {
            from: incarnation(3),
        };
        assert_eq!(*world.events::<IncarnationSwitched>(), [switched]);

        // read before the end of the tick clears it
        world.end_tick();
        assert!(world.events::<IncarnationSwitched>().is_empty());
        world.uid_allocator_mut().unwrap().set_spare(incarnation(5));
        world.uid_allocator_mut().unwrap().next = UidAllocator::SERIALS;
        assert_eq!(world.promote(world.spawn()), Some(uid(5, 0)));
        let switched = IncarnationSwitched {
            from: incarnation(4),
        };
        assert_eq!(*world.events::<IncarnationSwitched>(), [switched]);
    }

    #[test]
    #[should_panic(expected = "uid serials exhausted and no spare incarnation")]
    fn registry_promote_exhausted() {
        let registry = registry();
        {
            let mut allocator = registry.uid_allocator_mut().unwrap();
            allocator.next = UidAllocator::SERIALS;
            allocator.alloc();
            allocator.next = UidAllocator::SERIALS;
        }

        let e0 = registry.spawn();
        registry.promote(e0);
    }

    #[test]
    fn registry_despawn_forgets_mapping() {
        let registry = registry();
        let e0 = registry.spawn();
        let u0 = registry.promote(e0).unwrap();

        assert!(registry.despawn(e0));
        assert!(!registry.despawn(e0));
        assert_eq!(registry.uid(e0), None);
        assert_eq!(registry.resolve(u0), None);

        // the recycled slot must not inherit the old id
        let recycled = registry.spawn();
        assert_eq!(recycled.index(), e0.index());
        assert_eq!(registry.uid(recycled), None);

        let u1 = registry.promote(recycled).unwrap();
        assert_ne!(u1, u0); // serials are never reused
        assert_eq!(registry.resolve(u1), Some(recycled));
    }

    #[test]
    fn registry_stale_handle() {
        let registry = registry();
        let e0 = registry.spawn();
        registry.despawn(e0);
        let recycled = registry.spawn();
        let u = registry.promote(recycled).unwrap();

        // same index, old generation: must not see the new entity's id
        assert_eq!(registry.uid(e0), None);
        assert_eq!(registry.promote(e0), None);
        assert!(!registry.despawn(e0));
        assert_eq!(registry.resolve(u), Some(recycled));
    }

    #[test]
    fn registry_adopt() {
        let registry = registry();
        let foreign = uid(9, 42);

        let e0 = registry.adopt(foreign).unwrap();
        assert!(registry.is_alive(e0));
        assert_eq!(registry.uid(e0), Some(foreign));
        assert_eq!(registry.promote(e0), Some(foreign)); // keeps the adopted id
        assert_eq!(registry.resolve(foreign), Some(e0));
        // adopting never mints
        assert_eq!(
            registry.uid_allocator().unwrap().remaining(),
            UidAllocator::SERIALS
        );

        assert_eq!(registry.adopt(foreign), None); // already lives here
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn registry_migrate_round_trip() {
        let source = registry();
        let target = Registry::new();
        target.install_uid_allocator(UidAllocator::new(incarnation(5), incarnation(6)));

        let e = source.spawn();
        let uid = source.promote(e).unwrap();
        assert!(source.despawn(e));

        let moved = target.adopt(uid).unwrap();
        assert_eq!(target.resolve(uid), Some(moved));
        assert_eq!(source.resolve(uid), None);

        // and back again: the id survives, the local handle does not
        assert!(target.despawn(moved));
        let back = source.adopt(uid).unwrap();
        assert_ne!(back, e);
        assert_eq!(source.uid(back), Some(uid));
    }

    #[test]
    fn registry_replica_flags() {
        let registry = registry();
        let owned = registry.spawn();
        let replica = registry.adopt(uid(9, 1)).unwrap();
        registry.set_replica(replica, true);

        assert!(!registry.is_replica(owned));
        assert!(registry.is_replica(replica));
        assert!(registry.replicas().contains(replica));

        // promoted by a migration
        registry.set_replica(replica, false);
        assert!(!registry.is_replica(replica));

        // a recycled slot starts owned
        registry.set_replica(replica, true);
        assert!(registry.despawn(replica));
        assert!(!registry.is_replica(replica));
        let recycled = registry.spawn();
        assert_eq!(recycled.index(), replica.index());
        assert!(!registry.is_replica(recycled));

        // a dead entity is never marked
        registry.set_replica(replica, true);
        assert!(!registry.is_replica(recycled));
    }

    #[test]
    fn registry_borrows_apart() {
        let registry = registry();
        let e0 = registry.spawn();
        registry.set_cell(e0, Cell::new(1));

        // spawning only borrows the allocator
        let replicas = registry.replicas();
        let cells = registry.cell_index();
        let e1 = registry.spawn();
        assert!(!replicas.contains(e1));
        assert_eq!(cells.entities(Cell::new(1)), &[e0]);
    }

    #[test]
    fn registry_cells() {
        let registry = registry();
        let (a, b) = (Cell::new(1), Cell::new(2));
        let e0 = registry.spawn();
        let e1 = registry.spawn();
        assert_eq!(registry.cell(e0), None);

        registry.set_cell(e0, a);
        registry.set_cell(e1, a);
        assert_eq!(registry.cell(e0), Some(a));
        assert_eq!(registry.cell_index().entities(a), &[e0, e1]);

        registry.set_cell(e0, b);
        assert_eq!(registry.cell(e0), Some(b));
        assert_eq!(registry.cell_index().entities(a), &[e1]);
        assert_eq!(registry.cell_index().entities(b), &[e0]);

        // a dead entity gets no cell
        assert!(registry.despawn(e1));
        registry.set_cell(e1, b);
        assert_eq!(registry.cell(e1), None);
        assert!(registry.cell_index().entities(a).is_empty());
        assert_eq!(registry.cell_index().entities(b), &[e0]);

        // the recycled slot starts with none
        let recycled = registry.spawn();
        assert_eq!(recycled.index(), e1.index());
        assert_eq!(registry.cell(recycled), None);
        assert!(!registry.cell_index().contains(b, recycled));
    }

    #[test]
    fn registry_tick_start_cell() {
        let mut registry = registry();
        let (a, b, c) = (Cell::new(1), Cell::new(2), Cell::new(3));
        let e0 = registry.spawn();
        let e1 = registry.spawn();

        registry.set_cell(e0, a);
        assert_eq!(registry.tick_start_cell(e0), None); // entered
        registry.end_tick();
        assert_eq!(registry.tick_start_cell(e0), Some(a));

        // moved and back, then on: the start stays until the tick ends
        registry.set_cell(e0, b);
        registry.set_cell(e0, a);
        registry.set_cell(e0, c);
        registry.set_cell(e1, b);
        assert_eq!(registry.tick_start_cell(e0), Some(a));
        assert_eq!(registry.tick_start_cell(e1), None);
        registry.end_tick();
        assert_eq!(registry.tick_start_cell(e0), Some(c));
        assert_eq!(registry.tick_start_cell(e1), Some(b));

        assert!(registry.despawn(e0));
        assert_eq!(registry.tick_start_cell(e0), None);
        let recycled = registry.spawn();
        assert_eq!(registry.tick_start_cell(recycled), None);
        registry.end_tick();
        assert_eq!(registry.tick_start_cell(recycled), None);
    }

    #[test]
    fn registry_despawn_log() {
        let mut registry = registry();
        let cell = Cell::new(4);
        let replica = registry.adopt(uid(9, 1)).unwrap();
        let unnamed = registry.spawn();
        let entered = registry.spawn();
        let owned = registry.spawn();
        let uid_only = registry.spawn();
        registry.set_replica(replica, true);
        for entity in [replica, unnamed, owned] {
            registry.set_cell(entity, cell);
        }
        for entity in [entered, owned, uid_only] {
            registry.promote(entity);
        }
        registry.end_tick();
        registry.set_cell(entered, cell);

        let mut log = Vec::new();
        let all = [replica, unnamed, entered, owned, uid_only];
        assert_eq!(registry.despawn_batch(&all, &mut log), 5);
        // only the owned entity with a uid and a cell before this tick
        assert_eq!(
            log,
            [Despawned {
                uid: uid(3, 1),
                cell
            }]
        );
        assert!(registry.cell_index().entities(cell).is_empty());
    }
}
