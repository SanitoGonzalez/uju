use std::collections::HashMap;
use std::fmt;

use crate::ecs::entity::Entity;
use crate::ecs::hash::FoldBuildHasher;
use crate::ecs::storage::join::{Entities, Joinable};

/// A cell of the world's grid. Every replicated entity is in exactly one.
///
/// It is not a component: the [`Registry`](crate::ecs::entity::Registry) keeps
/// an entity's cell, and only uju sets it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Cell(u32);

impl Cell {
    #[inline]
    pub const fn new(id: u32) -> Self {
        Self(id)
    }

    #[inline(always)]
    pub const fn id(self) -> u32 {
        self.0
    }
}

impl fmt::Debug for Cell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Cell({})", self.0)
    }
}

impl fmt::Display for Cell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

const NONE: u32 = u32::MAX;

/// Per [`Cell`], a dense list of the entities in it, and per entity, its
/// position in that list, so moving or dropping an entity is a `swap_remove`.
#[derive(Default)]
pub struct CellIndex {
    lists: HashMap<Cell, Vec<Entity>, FoldBuildHasher>,
    positions: Vec<u32>,
}

impl CellIndex {
    /// The entities in `cell`, in no promised order.
    pub fn entities(&self, cell: Cell) -> &[Entity] {
        self.lists.get(&cell).map_or(&[], Vec::as_slice)
    }

    /// The cells holding at least one entity, in no promised order.
    pub fn cells(&self) -> impl Iterator<Item = Cell> + '_ {
        self.lists.keys().copied()
    }

    pub fn contains(&self, cell: Cell, entity: Entity) -> bool {
        Self::listed(self.entities(cell), &self.positions, entity)
    }

    #[inline]
    fn listed(list: &[Entity], positions: &[u32], entity: Entity) -> bool {
        positions
            .get(entity.index_sparse())
            .and_then(|&position| list.get(position as usize))
            == Some(&entity)
    }

    pub(crate) fn insert(&mut self, cell: Cell, entity: Entity) {
        let index = entity.index_sparse();
        if index >= self.positions.len() {
            self.positions.resize(index + 1, NONE);
        }
        let list = self.lists.entry(cell).or_default();
        self.positions[index] = list.len() as u32;
        list.push(entity);
    }

    pub(crate) fn remove(&mut self, cell: Cell, entity: Entity) {
        let Some(list) = self.lists.get_mut(&cell) else {
            return;
        };
        if !Self::listed(list, &self.positions, entity) {
            return;
        }

        let position = std::mem::replace(&mut self.positions[entity.index_sparse()], NONE);
        list.swap_remove(position as usize);
        if let Some(&moved) = list.get(position as usize) {
            self.positions[moved.index_sparse()] = position;
        } else if list.is_empty() {
            self.lists.remove(&cell);
        }
    }

    pub(crate) fn join(&self, cell: Cell) -> CellJoin<'_> {
        CellJoin {
            list: self.entities(cell),
            positions: &self.positions,
        }
    }
}

/// One cell's list as a join term: it keeps the entities in the cell.
pub(crate) struct CellJoin<'a> {
    list: &'a [Entity],
    positions: &'a [u32],
}

// SAFETY: `entities` yields the cell's list, which holds each entity once;
// `get` hands out no references.
unsafe impl<'a> Joinable<'a> for CellJoin<'a> {
    type Item = ();

    #[inline]
    fn len(&self) -> usize {
        self.list.len()
    }

    #[inline]
    fn entities(&self) -> Entities<'a> {
        self.list.iter().copied()
    }

    #[inline]
    unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
        CellIndex::listed(self.list, self.positions, entity).then_some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::entity::{EntityGeneration, EntityIndex};

    fn entity(index: u32) -> Entity {
        Entity::new(
            EntityIndex::from_bits(index),
            EntityGeneration::from_bits(1),
        )
    }

    #[test]
    fn cell_index_swap_remove() {
        let mut index = CellIndex::default();
        let (a, b) = (Cell::new(1), Cell::new(2));
        for i in 0..4 {
            index.insert(a, entity(i));
        }
        index.insert(b, entity(9));

        index.remove(a, entity(1));
        assert_eq!(index.entities(a), &[entity(0), entity(3), entity(2)]);
        assert!(index.contains(a, entity(3)));
        assert!(!index.contains(a, entity(1)));
        assert!(!index.contains(b, entity(3)));

        // a miss changes nothing
        index.remove(b, entity(3));
        index.remove(a, entity(1));
        assert_eq!(index.entities(a).len(), 3);
        assert_eq!(index.entities(b), &[entity(9)]);

        // the moved entity's position followed it
        index.remove(a, entity(3));
        assert_eq!(index.entities(a), &[entity(0), entity(2)]);
        index.remove(a, entity(2));
        index.remove(a, entity(0));
        assert!(index.entities(a).is_empty());
        assert_eq!(index.cells().collect::<Vec<_>>(), [b]);
    }

    #[test]
    fn cell_index_stale_entity() {
        let mut index = CellIndex::default();
        let cell = Cell::new(0);
        index.insert(cell, entity(0));

        // same index, other generation
        let stale = Entity::new(EntityIndex::from_bits(0), EntityGeneration::from_bits(3));
        assert!(!index.contains(cell, stale));
        index.remove(cell, stale);
        assert_eq!(index.entities(cell), &[entity(0)]);
    }
}
