use std::fmt;

const NODE_BITS: u8 = 10;
const SHARD_BITS: u8 = 10;
const GENERATION_BITS: u8 = 20;
const INDEX_BITS: u8 = 24;

const _: () = assert!(NODE_BITS + SHARD_BITS + GENERATION_BITS + INDEX_BITS == u64::BITS as u8);
const _: () = assert!(NODE_BITS <= u16::BITS as u8);
const _: () = assert!(SHARD_BITS <= u16::BITS as u8);
const _: () = assert!(GENERATION_BITS <= u32::BITS as u8);
const _: () = assert!(INDEX_BITS <= u32::BITS as u8);

const INDEX_SHIFT: u8 = 0;
const GENERATION_SHIFT: u8 = INDEX_SHIFT + INDEX_BITS;
const SHARD_SHIFT: u8 = GENERATION_SHIFT + GENERATION_BITS;
const NODE_SHIFT: u8 = SHARD_SHIFT + SHARD_BITS;

const NODE_MASK: u64 = (1 << NODE_BITS) - 1;
const SHARD_MASK: u64 = (1 << SHARD_BITS) - 1;
const GENERATION_MASK: u64 = (1 << GENERATION_BITS) - 1;
const INDEX_MASK: u64 = (1 << INDEX_BITS) - 1;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Entity(u64);

impl Entity {
    pub const MAX_NODE: u16 = NODE_MASK as u16;
    pub const MAX_SHARD: u16 = SHARD_MASK as u16;
    pub const MAX_GENERATION: u32 = GENERATION_MASK as u32;
    pub const MAX_INDEX: u32 = INDEX_MASK as u32;

    pub(crate) const fn new(node: u16, shard: u16, generation: u32, index: u32) -> Self {
        assert!(node <= Self::MAX_NODE);
        assert!(shard <= Self::MAX_SHARD);
        assert!(generation <= Self::MAX_GENERATION);
        assert!(index <= Self::MAX_INDEX);

        let entity: u64 = (node as u64) << NODE_SHIFT
            | (shard as u64) << SHARD_SHIFT
            | (generation as u64) << GENERATION_SHIFT
            | (index as u64) << INDEX_SHIFT;
        Self(entity)
    }

    #[inline]
    pub const fn node(self) -> u16 {
        ((self.0 >> NODE_SHIFT) & NODE_MASK) as u16
    }

    #[inline]
    pub const fn shard(self) -> u16 {
        ((self.0 >> SHARD_SHIFT) & SHARD_MASK) as u16
    }

    #[inline]
    pub const fn generation(self) -> u32 {
        ((self.0 >> GENERATION_SHIFT) & GENERATION_MASK) as u32
    }

    #[inline]
    pub const fn index(self) -> usize {
        ((self.0 >> INDEX_SHIFT) & INDEX_MASK) as usize
    }

    #[inline]
    pub const fn index_u32(self) -> u32 {
        ((self.0 >> INDEX_SHIFT) & INDEX_MASK) as u32
    }

    /// Same node and shard, bumped generation, wrapping on overflow.
    pub(crate) const fn recycled(self) -> Self {
        let generation = (self.generation() + 1) & Self::MAX_GENERATION;
        Self::new(self.node(), self.shard(), generation, self.index_u32())
    }

    #[inline]
    pub const fn to_bits(self) -> u64 {
        self.0
    }

    #[inline]
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }
}

impl fmt::Debug for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entity")
            .field("node", &self.node())
            .field("shard", &self.shard())
            .field("generation", &self.generation())
            .field("index", &self.index())
            .finish()
    }
}

impl fmt::Display for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}v{}",
            self.node(),
            self.shard(),
            self.index(),
            self.generation()
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    use super::*;

    fn hash(entity: Entity) -> u64 {
        let mut hasher = DefaultHasher::new();
        entity.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn fields_roundtrip() {
        let entity = Entity::new(3, 7, 42, 1234);

        assert_eq!(entity.node(), 3);
        assert_eq!(entity.shard(), 7);
        assert_eq!(entity.generation(), 42);
        assert_eq!(entity.index(), 1234);
    }

    #[test]
    fn zero_fields_roundtrip() {
        let entity = Entity::new(0, 0, 0, 0);

        assert_eq!(entity.to_bits(), 0);
        assert_eq!(entity.node(), 0);
        assert_eq!(entity.shard(), 0);
        assert_eq!(entity.generation(), 0);
        assert_eq!(entity.index(), 0);
    }

    #[test]
    fn max_fields_roundtrip() {
        let entity = Entity::new(
            Entity::MAX_NODE,
            Entity::MAX_SHARD,
            Entity::MAX_GENERATION,
            Entity::MAX_INDEX,
        );

        assert_eq!(entity.to_bits(), u64::MAX);
        assert_eq!(entity.node(), Entity::MAX_NODE);
        assert_eq!(entity.shard(), Entity::MAX_SHARD);
        assert_eq!(entity.generation(), Entity::MAX_GENERATION);
        assert_eq!(entity.index_u32(), Entity::MAX_INDEX);
    }

    #[test]
    fn fields_do_not_overlap() {
        let node = Entity::new(Entity::MAX_NODE, 0, 0, 0);
        assert_eq!(node.to_bits(), NODE_MASK << NODE_SHIFT);
        assert_eq!((node.shard(), node.generation(), node.index()), (0, 0, 0));

        let shard = Entity::new(0, Entity::MAX_SHARD, 0, 0);
        assert_eq!(shard.to_bits(), SHARD_MASK << SHARD_SHIFT);
        assert_eq!((shard.node(), shard.generation(), shard.index()), (0, 0, 0));

        let generation = Entity::new(0, 0, Entity::MAX_GENERATION, 0);
        assert_eq!(generation.to_bits(), GENERATION_MASK << GENERATION_SHIFT);
        assert_eq!(
            (generation.node(), generation.shard(), generation.index()),
            (0, 0, 0)
        );

        let index = Entity::new(0, 0, 0, Entity::MAX_INDEX);
        assert_eq!(index.to_bits(), INDEX_MASK << INDEX_SHIFT);
        assert_eq!((index.node(), index.shard(), index.generation()), (0, 0, 0));
    }

    #[test]
    fn bit_layout_is_stable() {
        let entity = Entity::new(1, 1, 1, 1);
        let expected = 1 << 54 | 1 << 44 | 1 << 24 | 1;

        assert_eq!(entity.to_bits(), expected);
    }

    #[test]
    fn bits_roundtrip() {
        let entity = Entity::new(9, 21, 500_000, 16_000_000);

        assert_eq!(Entity::from_bits(entity.to_bits()), entity);
    }

    #[test]
    fn recycled_bumps_generation_only() {
        let entity = Entity::new(2, 5, 8, 99);
        let recycled = entity.recycled();

        assert_eq!(recycled.generation(), 9);
        assert_eq!(recycled.node(), entity.node());
        assert_eq!(recycled.shard(), entity.shard());
        assert_eq!(recycled.index(), entity.index());
        assert_ne!(recycled, entity);
    }

    #[test]
    fn recycled_wraps_generation() {
        let entity = Entity::new(2, 5, Entity::MAX_GENERATION, 99);

        assert_eq!(entity.recycled().generation(), 0);
    }

    #[test]
    fn equality_and_hash_follow_all_fields() {
        let entity = Entity::new(1, 2, 3, 4);

        assert_eq!(entity, Entity::new(1, 2, 3, 4));
        assert_eq!(hash(entity), hash(Entity::new(1, 2, 3, 4)));

        for other in [
            Entity::new(0, 2, 3, 4),
            Entity::new(1, 0, 3, 4),
            Entity::new(1, 2, 0, 4),
            Entity::new(1, 2, 3, 0),
        ] {
            assert_ne!(entity, other);
            assert_ne!(hash(entity), hash(other));
        }
    }

    #[test]
    fn formatting() {
        let entity = Entity::new(1, 2, 3, 4);

        assert_eq!(entity.to_string(), "1:2:4v3");
        assert_eq!(
            format!("{entity:?}"),
            "Entity { node: 1, shard: 2, generation: 3, index: 4 }"
        );
    }

    #[test]
    #[should_panic]
    fn node_out_of_range_panics() {
        Entity::new(Entity::MAX_NODE + 1, 0, 0, 0);
    }

    #[test]
    #[should_panic]
    fn shard_out_of_range_panics() {
        Entity::new(0, Entity::MAX_SHARD + 1, 0, 0);
    }

    #[test]
    #[should_panic]
    fn generation_out_of_range_panics() {
        Entity::new(0, 0, Entity::MAX_GENERATION + 1, 0);
    }

    #[test]
    #[should_panic]
    fn index_out_of_range_panics() {
        Entity::new(0, 0, 0, Entity::MAX_INDEX + 1);
    }
}
