pub mod allocator;

use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(transparent)]
pub struct EntityIndex(u32);

impl EntityIndex {
    pub const NULL: Self = Self(u32::MAX);

    #[inline]
    pub const fn new(index: u32) -> Self {
        assert!(index < u32::MAX);
        Self(index)
    }

    #[inline(always)]
    pub const fn to_bits(self) -> u32 {
        self.0
    }

    #[inline(always)]
    pub const fn from_bits(bits: u32) -> Self {
        assert!(bits < u32::MAX);
        Self(bits)
    }

    #[inline(always)]
    pub fn next(&self) -> Self {
        let index = self.0.wrapping_add(1);
        Self(if index < u32::MAX { index } else { 0 })
    }
}

impl fmt::Display for EntityIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(transparent)]
pub struct EntityGeneration(u32);

impl EntityGeneration {
    #[inline]
    pub const fn new(generation: u32) -> Self {
        Self(generation)
    }

    #[inline(always)]
    pub const fn to_bits(self) -> u32 {
        self.0
    }

    #[inline(always)]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    #[inline(always)]
    pub fn next(&self) -> Self {
        Self(self.0.wrapping_add(1))
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
    #[cfg(target_endian = "little")]
    index: EntityIndex,
    generation: EntityGeneration,
    #[cfg(target_endian = "big")]
    index: EntityIndex,
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
        write!(f, "{}v{}", self.index, self.generation,)
    }
}
