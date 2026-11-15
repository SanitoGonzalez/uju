use std::collections::HashMap;
use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};

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
/// It never asks for incarnations itself. Whoever installed it reads the incarnations it
/// switched away from with [`take_retired`](Self::take_retired), and hands it the next
/// spare with [`set_spare`](Self::set_spare).
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
    pub fn take_retired(&mut self) -> Vec<NonZeroU32> {
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

/// Owns a world's local entities and their mapping to [`Uid`]s.
///
/// A local [`Entity`] never leaves its world. A [`Uid`] is minted lazily, only once the
/// entity has to be named from outside (see [`promote`](Self::promote)), or adopted
/// from another world (see [`adopt`](Self::adopt)).
pub struct Registry {
    local: Allocator,
    allocator: Option<UidAllocator>,
    uids: Vec<Option<Uid>>,
    entities: HashMap<Uid, Entity, FoldBuildHasher>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            local: Allocator::new(),
            allocator: None,
            uids: Vec::new(),
            entities: HashMap::default(),
        }
    }

    pub fn len(&self) -> usize {
        self.local.len()
    }

    pub fn is_empty(&self) -> bool {
        self.local.is_empty()
    }

    /// The allocator [`promote`](Self::promote) mints from, if one is installed.
    pub fn uid_allocator(&self) -> Option<&UidAllocator> {
        self.allocator.as_ref()
    }

    /// The allocator [`promote`](Self::promote) mints from, if one is installed.
    pub fn uid_allocator_mut(&mut self) -> Option<&mut UidAllocator> {
        self.allocator.as_mut()
    }

    /// Installs the allocator [`promote`](Self::promote) mints from, returning the
    /// previous one.
    pub fn install_uid_allocator(&mut self, allocator: UidAllocator) -> Option<UidAllocator> {
        self.allocator.replace(allocator)
    }

    pub fn is_alive(&self, entity: Entity) -> bool {
        self.local.is_alive(entity)
    }

    /// Allocates a local entity. It has no [`Uid`] until promoted.
    pub fn spawn(&mut self) -> Entity {
        self.local.alloc()
    }

    /// Frees `entity` and forgets its [`Uid`], if any.
    pub fn despawn(&mut self, entity: Entity) -> bool {
        if !self.local.dealloc(entity) {
            return false;
        }

        // the slot will be recycled, so its mapping must not outlive the entity
        if let Some(uid) = self
            .uids
            .get_mut(entity.index_sparse())
            .and_then(Option::take)
        {
            self.entities.remove(&uid);
        }
        true
    }

    /// The [`Uid`] of `entity`, if it is alive and has one.
    pub fn uid(&self, entity: Entity) -> Option<Uid> {
        if !self.is_alive(entity) {
            return None;
        }
        self.uids.get(entity.index_sparse()).copied().flatten()
    }

    /// The [`Uid`] of `entity`, minting one on first use. `None` if `entity` is dead, or
    /// if it has no uid and no allocator is installed.
    ///
    /// Panics if the installed allocator's current incarnation has run out and it has no
    /// spare (see [`UidAllocator::alloc`]).
    pub fn promote(&mut self, entity: Entity) -> Option<Uid> {
        if !self.is_alive(entity) {
            return None;
        }

        let index = entity.index_sparse();
        if let Some(uid) = self.uids.get(index).copied().flatten() {
            return Some(uid);
        }

        let uid = self.allocator.as_mut()?.alloc();
        self.bind(entity, uid);
        Some(uid)
    }

    /// The local entity carrying `uid`, if this world holds it.
    pub fn resolve(&self, uid: Uid) -> Option<Entity> {
        self.entities.get(&uid).copied()
    }

    /// Spawns a local entity for an existing `uid`, e.g. one migrating in or replicated
    /// from another world. `None` if `uid` already has a local entity here.
    pub fn adopt(&mut self, uid: Uid) -> Option<Entity> {
        if self.entities.contains_key(&uid) {
            return None;
        }

        let entity = self.local.alloc();
        self.bind(entity, uid);
        Some(entity)
    }

    fn bind(&mut self, entity: Entity, uid: Uid) {
        let index = entity.index_sparse();
        if index >= self.uids.len() {
            self.uids.resize(index + 1, None);
        }
        self.uids[index] = Some(uid);
        self.entities.insert(uid, entity);
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
        let mut registry = Registry::new();
        registry.install_uid_allocator(allocator());
        registry
    }

    #[test]
    fn registry_spawn_is_local_only() {
        let mut registry = registry();
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
        let mut registry = registry();
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
        let mut registry = registry();
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
        let mut registry = Registry::new();
        let e0 = registry.spawn();
        assert_eq!(registry.promote(e0), None);
        assert_eq!(registry.uid(e0), None);

        // an adopted uid is still reported
        let e1 = registry.adopt(uid(9, 42)).unwrap();
        assert_eq!(registry.promote(e1), Some(uid(9, 42)));
    }

    #[test]
    fn registry_promote_switches() {
        let mut registry = registry();
        let e0 = registry.spawn();
        let u0 = registry.promote(e0).unwrap();

        registry.uid_allocator_mut().unwrap().next = UidAllocator::SERIALS;
        let e1 = registry.spawn();
        assert_eq!(registry.promote(e1), Some(uid(4, 0)));
        assert_eq!(registry.promote(e0), Some(u0)); // minted ids stay

        let allocator = registry.uid_allocator_mut().unwrap();
        assert_eq!(allocator.take_retired(), [incarnation(3)]);
        assert_eq!(allocator.set_spare(incarnation(5)), None);
    }

    #[test]
    #[should_panic(expected = "uid serials exhausted and no spare incarnation")]
    fn registry_promote_exhausted() {
        let mut registry = registry();
        let allocator = registry.uid_allocator_mut().unwrap();
        allocator.next = UidAllocator::SERIALS;
        allocator.alloc();
        allocator.next = UidAllocator::SERIALS;

        let e0 = registry.spawn();
        registry.promote(e0);
    }

    #[test]
    fn registry_despawn_forgets_mapping() {
        let mut registry = registry();
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
        let mut registry = registry();
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
        let mut registry = registry();
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
        let mut source = registry();
        let mut target = Registry::new();
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
}
