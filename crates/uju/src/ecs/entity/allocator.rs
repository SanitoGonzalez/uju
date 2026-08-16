use crate::ecs::entity::{Entity, EntityGeneration, EntityIndex};
use crate::ecs::storage::sparse_bitset::SparseBitset;

pub struct Allocator {
    generations: Vec<EntityGeneration>,
    frees: Vec<EntityIndex>,
    replicas: SparseBitset,
    alive: usize,
}

impl Allocator {
    pub fn new() -> Self {
        Self {
            generations: Vec::new(),
            frees: Vec::new(),
            replicas: SparseBitset::new(),
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

    pub fn alloc_replica(&mut self) -> Entity {
        let entity = self.alloc();
        self.replicas.insert(entity.index_sparse());
        entity
    }

    pub fn try_alloc(&mut self) -> Option<Entity> {
        let entity = match self.frees.pop() {
            Some(index) => {
                let generation = &mut self.generations[index.to_bits() as usize];
                *generation = generation.next(); // even -> odd: alive
                Entity::new(index, *generation)
            }
            None => {
                if self.generations.len() >= EntityIndex::NULL.to_bits() as usize {
                    return None;
                }
                let index = EntityIndex::new(self.generations.len() as u32);
                let generation = EntityGeneration::new(1);
                self.generations.push(generation);
                Entity::new(index, generation)
            }
        };
        self.alive += 1;
        Some(entity)
    }

    pub fn try_alloc_replica(&mut self) -> Option<Entity> {
        let entity = self.try_alloc()?;
        self.replicas.insert(entity.index_sparse());
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

        self.replicas.remove(entity.index_sparse());

        self.alive -= 1;
        true
    }

    pub fn is_alive(&self, entity: Entity) -> bool {
        self.generations
            .get(entity.index_sparse())
            .is_some_and(|&g| g == entity.generation() && g.to_bits() & 1 == 1)
    }

    pub fn is_replica(&self, entity: Entity) -> bool {
        self.replicas.contains(entity.index_sparse())
    }
}

impl Default for Allocator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn allocator_alloc_replica() {
        let mut allocator = Allocator::new();
        let local = allocator.alloc();
        let replica = allocator.alloc_replica();

        assert_ne!(local.index(), replica.index()); // replicas draw from the same index space
        assert!(allocator.is_alive(local));
        assert!(allocator.is_alive(replica));
        assert_eq!(allocator.len(), 2); // and count toward the alive total

        assert!(!allocator.is_replica(local));
        assert!(allocator.is_replica(replica));
    }

    #[test]
    fn allocator_try_alloc_replica() {
        let mut allocator = Allocator::new();

        let replica = allocator.try_alloc_replica().expect("fresh slot");
        assert_eq!(replica.index(), EntityIndex::from_bits(0));
        assert!(allocator.is_alive(replica));
        assert!(allocator.is_replica(replica));

        assert!(allocator.dealloc(replica));
        let recycled = allocator.try_alloc_replica().expect("recycled slot");
        assert_eq!(recycled.index(), replica.index());
        assert!(allocator.is_replica(recycled));
    }

    #[test]
    fn allocator_replica_mixed() {
        let mut allocator = Allocator::new();
        let entities: Vec<_> = (0..8)
            .map(|i| {
                if i % 2 == 0 {
                    allocator.alloc()
                } else {
                    allocator.alloc_replica()
                }
            })
            .collect();

        for (i, &entity) in entities.iter().enumerate() {
            assert_eq!(allocator.is_replica(entity), i % 2 == 1);
        }
        assert_eq!(allocator.len(), 8);
    }

    #[test]
    fn allocator_replica_recycle() {
        let mut allocator = Allocator::new();
        let local = allocator.alloc();
        assert!(allocator.dealloc(local));

        // a recycled slot can come back as a replica
        let replica = allocator.alloc_replica();
        assert_eq!(replica.index(), local.index());
        assert!(allocator.is_replica(replica));

        // and dealloc must clear the bit, or the next tenant inherits it
        assert!(allocator.dealloc(replica));
        assert!(!allocator.is_replica(replica));

        let recycled = allocator.alloc();
        assert_eq!(recycled.index(), replica.index());
        assert!(!allocator.is_replica(recycled));
    }

    #[test]
    fn allocator_replica_unknown_entity() {
        let mut allocator = Allocator::new();
        let unknown = Entity::new(EntityIndex::from_bits(7), EntityGeneration::from_bits(1));

        assert!(!allocator.is_replica(unknown));

        // `is_replica` answers for the slot, not the handle, so a stale handle borrows
        // the answer of whoever holds its index now - callers must gate on `is_alive`
        let stale = allocator.alloc();
        assert!(allocator.dealloc(stale));
        let replica = allocator.alloc_replica();

        assert_eq!(replica.index(), stale.index());
        assert_ne!(replica, stale);
        assert!(!allocator.is_alive(stale));
        assert!(allocator.is_replica(stale));
    }

    #[test]
    fn allocator_replica_retire_exhausted_slot() {
        let mut allocator = Allocator::new();
        let replica = allocator.alloc_replica();

        // fast-forward the slot to its last usable alive generation
        let last_generation = EntityGeneration::from_bits(u32::MAX - 2);
        allocator.generations[replica.index_sparse()] = last_generation;
        let last = Entity::new(replica.index(), last_generation);
        assert!(allocator.is_replica(last));

        assert!(allocator.dealloc(last));
        assert!(allocator.frees.is_empty()); // retired instead of recycled
        assert!(!allocator.is_replica(last)); // and the replica bit went with it
    }
}
