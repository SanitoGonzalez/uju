use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroU32;

use crate::cluster::{node, shard};

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
}

impl Default for Allocator {
    fn default() -> Self {
        Self::new()
    }
}

/// Cluster-wide identity of an entity, stable across migrations. `node` and `shard`
/// name the creator, not the current owner - they only keep serials from colliding.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct UniversalId {
    serial: u64,
    node: node::Id,
    shard: shard::Id,
}

impl UniversalId {
    pub const BYTES: usize = 12;

    #[inline]
    pub const fn new(node: node::Id, shard: shard::Id, serial: u64) -> Self {
        Self {
            serial,
            node,
            shard,
        }
    }

    #[inline(always)]
    pub const fn serial(&self) -> u64 {
        self.serial
    }

    #[inline(always)]
    pub const fn node(&self) -> node::Id {
        self.node
    }

    #[inline(always)]
    pub const fn shard(&self) -> shard::Id {
        self.shard
    }
}

impl fmt::Debug for UniversalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for UniversalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.node, self.shard, self.serial)
    }
}

/// Mints [`UniversalId`]s for one shard. Counters are never reused.
pub struct UniversalAllocator {
    node: node::Id,
    shard: shard::Id,
    next: u64,
}

impl UniversalAllocator {
    /// Creates an allocator minting from `start`.
    ///
    /// Migrated entities can outlive their creator, so after a restart `start` must
    /// exceed every serial minted before - otherwise ids may alias live entities.
    /// Persist [`next_serial`](Self::next_serial) and pass it back here.
    pub fn new(node: node::Id, shard: shard::Id, start: u64) -> Self {
        Self {
            node,
            shard,
            next: start,
        }
    }

    /// The serial the next id will carry - the high-water mark to persist.
    pub fn next_serial(&self) -> u64 {
        self.next
    }

    pub fn alloc(&mut self) -> UniversalId {
        self.try_alloc().expect("universal id serial exhausted")
    }

    pub fn try_alloc(&mut self) -> Option<UniversalId> {
        let serial = self.next;
        self.next = serial.checked_add(1)?;
        Some(UniversalId::new(self.node, self.shard, serial))
    }
}

/// Owns the shard's local entities and their mapping to [`UniversalId`]s.
///
/// A local [`Entity`] never leaves the shard; a [`UniversalId`] is minted lazily, only
/// once the entity has to be addressed from outside (see [`promote`](Self::promote)).
pub struct Registry {
    local: Allocator,
    universal: UniversalAllocator,
    to_universal: Vec<Option<UniversalId>>,
    to_local: HashMap<UniversalId, Entity>,
}

impl Registry {
    pub fn new(universal: UniversalAllocator) -> Self {
        Self {
            local: Allocator::new(),
            universal,
            to_universal: Vec::new(),
            to_local: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.local.len()
    }

    pub fn is_empty(&self) -> bool {
        self.local.is_empty()
    }

    /// The serial high-water mark to persist - see [`UniversalAllocator::new`].
    pub fn next_serial(&self) -> u64 {
        self.universal.next_serial()
    }

    pub fn is_alive(&self, entity: Entity) -> bool {
        self.local.is_alive(entity)
    }

    /// Allocates a local entity. It has no [`UniversalId`] until promoted.
    pub fn spawn(&mut self) -> Entity {
        self.local.alloc()
    }

    /// Frees `entity` and forgets its [`UniversalId`], if any.
    pub fn despawn(&mut self, entity: Entity) -> bool {
        if !self.local.dealloc(entity) {
            return false;
        }

        // the slot will be recycled, so its mapping must not outlive the entity
        if let Some(uid) = self
            .to_universal
            .get_mut(entity.index_sparse())
            .and_then(Option::take)
        {
            self.to_local.remove(&uid);
        }
        true
    }

    /// The [`UniversalId`] of `entity`, if it is alive and has been promoted.
    pub fn universal(&self, entity: Entity) -> Option<UniversalId> {
        if !self.is_alive(entity) {
            return None;
        }
        self.to_universal.get(entity.index_sparse()).copied().flatten()
    }

    /// The [`UniversalId`] of `entity`, minting one on first use. `None` if dead.
    pub fn promote(&mut self, entity: Entity) -> Option<UniversalId> {
        if !self.is_alive(entity) {
            return None;
        }

        let index = entity.index_sparse();
        if index >= self.to_universal.len() {
            self.to_universal.resize(index + 1, None);
        }

        let slot = &mut self.to_universal[index];
        if let Some(uid) = *slot {
            return Some(uid);
        }

        let uid = self.universal.alloc();
        *slot = Some(uid);
        self.to_local.insert(uid, entity);
        Some(uid)
    }

    /// The local entity carrying `uid`, if it lives on this shard.
    pub fn resolve(&self, uid: UniversalId) -> Option<Entity> {
        self.to_local.get(&uid).copied()
    }

    /// Spawns a local entity for an existing `uid`, e.g. one migrating in.
    /// `None` if `uid` already has a local entity here.
    pub fn adopt(&mut self, uid: UniversalId) -> Option<Entity> {
        if self.to_local.contains_key(&uid) {
            return None;
        }

        let entity = self.local.alloc();
        let index = entity.index_sparse();
        if index >= self.to_universal.len() {
            self.to_universal.resize(index + 1, None);
        }

        self.to_universal[index] = Some(uid);
        self.to_local.insert(uid, entity);
        Some(entity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn universal_allocator_alloc() {
        let mut allocator = UniversalAllocator::new(3, 7, 0);
        let u0 = allocator.alloc();
        let u1 = allocator.alloc();

        assert_eq!(u0, UniversalId::new(3, 7, 0));
        assert_eq!(u1, UniversalId::new(3, 7, 1));
        assert_eq!(allocator.next_serial(), 2);
    }

    #[test]
    fn universal_allocator_resume() {
        let mut before = UniversalAllocator::new(3, 7, 0);
        let minted = [before.alloc(), before.alloc()];

        // a restart resuming from the persisted high-water mark never re-mints
        let mut after = UniversalAllocator::new(3, 7, before.next_serial());
        let resumed = after.alloc();
        assert!(!minted.contains(&resumed));
        assert_eq!(resumed.serial(), 2);
    }

    #[test]
    fn universal_allocator_distinct_creators() {
        // creators count independently, so equal serials are the normal case
        let a = UniversalAllocator::new(3, 7, 0).alloc();
        let b = UniversalAllocator::new(3, 8, 0).alloc();
        let c = UniversalAllocator::new(4, 7, 0).alloc();

        assert_eq!(a.serial(), b.serial());
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
    }

    #[test]
    fn universal_allocator_exhausted() {
        let mut allocator = UniversalAllocator::new(3, 7, u64::MAX - 1);

        assert_eq!(
            allocator.try_alloc(),
            Some(UniversalId::new(3, 7, u64::MAX - 1))
        );
        assert_eq!(allocator.try_alloc(), None);
        assert_eq!(allocator.try_alloc(), None);
    }

    fn registry() -> Registry {
        Registry::new(UniversalAllocator::new(3, 7, 0))
    }

    #[test]
    fn registry_spawn_is_local_only() {
        let mut registry = registry();
        let e0 = registry.spawn();

        assert!(registry.is_alive(e0));
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.universal(e0), None);
        assert_eq!(registry.next_serial(), 0); // nothing minted yet
    }

    #[test]
    fn registry_promote() {
        let mut registry = registry();
        let e0 = registry.spawn();
        let e1 = registry.spawn();

        let u1 = registry.promote(e1).unwrap();
        assert_eq!(u1, UniversalId::new(3, 7, 0));
        assert_eq!(registry.promote(e1), Some(u1)); // idempotent, no re-mint
        assert_eq!(registry.next_serial(), 1);

        assert_eq!(registry.universal(e1), Some(u1));
        assert_eq!(registry.resolve(u1), Some(e1));
        assert_eq!(registry.universal(e0), None);
    }

    #[test]
    fn registry_promote_dead() {
        let mut registry = registry();
        let e0 = registry.spawn();
        assert!(registry.despawn(e0));

        assert_eq!(registry.promote(e0), None);
        assert_eq!(registry.next_serial(), 0);
    }

    #[test]
    fn registry_despawn_forgets_mapping() {
        let mut registry = registry();
        let e0 = registry.spawn();
        let u0 = registry.promote(e0).unwrap();

        assert!(registry.despawn(e0));
        assert!(!registry.despawn(e0));
        assert_eq!(registry.universal(e0), None);
        assert_eq!(registry.resolve(u0), None);

        // the recycled slot must not inherit the old id
        let recycled = registry.spawn();
        assert_eq!(recycled.index(), e0.index());
        assert_eq!(registry.universal(recycled), None);

        let u1 = registry.promote(recycled).unwrap();
        assert_ne!(u1, u0); // serials are never reused
        assert_eq!(registry.resolve(u1), Some(recycled));
    }

    #[test]
    fn registry_stale_handle() {
        let mut registry = registry();
        let e0 = registry.spawn();
        registry.despawn(e0);
        let recycled = registry.spawn();
        let u = registry.promote(recycled).unwrap();

        // same index, old generation: must not see the new entity's id
        assert_eq!(registry.universal(e0), None);
        assert_eq!(registry.promote(e0), None);
        assert!(!registry.despawn(e0));
        assert_eq!(registry.resolve(u), Some(recycled));
    }

    #[test]
    fn registry_adopt() {
        let mut registry = registry();
        let foreign = UniversalId::new(9, 1, 42);

        let e0 = registry.adopt(foreign).unwrap();
        assert!(registry.is_alive(e0));
        assert_eq!(registry.universal(e0), Some(foreign));
        assert_eq!(registry.promote(e0), Some(foreign)); // keeps the adopted id
        assert_eq!(registry.resolve(foreign), Some(e0));
        assert_eq!(registry.next_serial(), 0); // adopting never mints

        assert_eq!(registry.adopt(foreign), None); // already lives here
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn registry_migrate_round_trip() {
        let mut source = registry();
        let mut target = Registry::new(UniversalAllocator::new(3, 8, 0));

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
        assert_eq!(source.universal(back), Some(uid));
    }
}
