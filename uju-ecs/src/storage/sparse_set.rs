use crate::Entity;

/// A sparse-set map from [`Entity`] to a component of type `T`: O(1) insert,
/// remove, and lookup, plus cache-friendly contiguous iteration over components.
pub struct SparseSet<T> {
    entities: Vec<Entity>,
    components: Vec<T>,
    sparse: Vec<usize>,
}

impl<T> SparseSet<T> {
    pub fn new() -> Self {
        Self {
            entities: Vec::new(),
            components: Vec::new(),
            sparse: Vec::new(),
        }
    }

    /// Dense slot holding `entity`, if it is present.
    fn dense_index(&self, entity: Entity) -> Option<usize> {
        let &idx = self.sparse.get(entity)?;
        // The sparse slot may be stale (left over from a removed entity or never
        // written). The membership check confirms it really points back at us.
        if idx < self.entities.len() && self.entities[idx] == entity {
            Some(idx)
        } else {
            None
        }
    }

    pub fn contains(&self, entity: Entity) -> bool {
        self.dense_index(entity).is_some()
    }

    /// Inserts or overwrites the component for `entity`. Returns the previous
    /// component if one was present.
    pub fn insert(&mut self, entity: Entity, component: T) -> Option<T> {
        if let Some(idx) = self.dense_index(entity) {
            return Some(std::mem::replace(&mut self.components[idx], component));
        }
        if entity >= self.sparse.len() {
            self.sparse.resize(entity + 1, 0);
        }
        self.sparse[entity] = self.entities.len();
        self.entities.push(entity);
        self.components.push(component);
        None
    }

    pub fn get(&self, entity: Entity) -> Option<&T> {
        let idx = self.dense_index(entity)?;
        Some(&self.components[idx])
    }

    pub fn get_mut(&mut self, entity: Entity) -> Option<&mut T> {
        let idx = self.dense_index(entity)?;
        Some(&mut self.components[idx])
    }

    /// Removes and returns the component for `entity`, if present.
    pub fn remove(&mut self, entity: Entity) -> Option<T> {
        let idx = self.dense_index(entity)?;
        // Swap the slot with the last one so the dense arrays stay gap-free, then
        // fix up the moved entity's sparse entry. Both arrays move in lockstep.
        let last = self.entities.len() - 1;
        self.entities.swap(idx, last);
        self.components.swap(idx, last);
        self.entities.pop();
        let component = self.components.pop();
        if idx != last {
            let moved = self.entities[idx];
            self.sparse[moved] = idx;
        }
        component
    }

    pub fn clear(&mut self) {
        // Only the dense arrays need clearing; stale sparse entries are caught by
        // the membership check in `dense_index`.
        self.entities.clear();
        self.components.clear();
    }

    pub fn len(&self) -> usize {
        self.entities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// Iterate `(entity, &component)` pairs in dense order.
    pub fn iter(&self) -> impl Iterator<Item = (Entity, &T)> {
        self.entities.iter().copied().zip(self.components.iter())
    }

    /// Iterate `(entity, &mut component)` pairs in dense order.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Entity, &mut T)> {
        self.entities.iter().copied().zip(self.components.iter_mut())
    }

    /// The contiguous component array — cache-friendly for systems that don't
    /// need the entity ids.
    pub fn components(&self) -> &[T] {
        &self.components
    }
}

impl<T> Default for SparseSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_remove() {
        let mut set = SparseSet::<&str>::new();
        assert_eq!(set.insert(3, "a"), None);
        assert_eq!(set.insert(1, "b"), None);
        assert_eq!(set.insert(3, "a2"), Some("a")); // overwrite returns old
        assert_eq!(set.get(3), Some(&"a2"));
        assert_eq!(set.get(1), Some(&"b"));
        assert_eq!(set.get(2), None);
        assert_eq!(set.len(), 2);

        assert_eq!(set.remove(3), Some("a2"));
        assert_eq!(set.remove(3), None); // already gone
        assert!(!set.contains(3));
        assert!(set.contains(1));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn swap_remove_keeps_arrays_aligned() {
        let mut set = SparseSet::<u32>::new();
        for (e, c) in [(10, 100), (20, 200), (30, 300), (40, 400)] {
            set.insert(e, c);
        }
        set.remove(20); // entity 40 swaps into 20's old slot
        for (e, c) in [(10, 100), (30, 300), (40, 400)] {
            assert_eq!(set.get(e), Some(&c));
        }
        assert!(!set.contains(20));
    }

    #[test]
    fn get_mut_and_iter() {
        let mut set = SparseSet::<u32>::new();
        set.insert(5, 1);
        set.insert(7, 2);
        *set.get_mut(5).unwrap() += 10;
        let mut pairs: Vec<_> = set.iter().map(|(e, &c)| (e, c)).collect();
        pairs.sort();
        assert_eq!(pairs, vec![(5, 11), (7, 2)]);
    }

    #[test]
    fn clear_empties() {
        let mut set = SparseSet::<u32>::new();
        set.insert(5, 50);
        set.clear();
        assert!(!set.contains(5));
        assert!(set.is_empty());
    }
}
