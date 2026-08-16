use std::iter::Copied;
use std::slice;

use crate::ecs::entity::Entity;

pub type Entities<'a> = Copied<slice::Iter<'a, Entity>>;

/// A term of a [`join`]: shared (`&SparseSet<T>`), mutable
/// (`&mut SparseSet<T>`), or a tuple of terms.
///
/// # Safety
///
/// Implementations must guarantee that [`entities`](Joinable::entities)
/// yields each entity at most once. [`join`] relies on this to uphold
/// [`get`](Joinable::get)'s at-most-once contract, without which mutable
/// terms would hand out aliasing `&mut` borrows through safe code.
pub unsafe trait Joinable<'a> {
    type Item;

    fn len(&self) -> usize;
    fn entities(&self) -> Entities<'a>;

    /// Probes this term for `entity`.
    ///
    /// # Safety
    ///
    /// Each entity may be probed at most once per term for the whole `'a`.
    /// A second probe of the same entity would let a mutable term return a
    /// second `&'a mut` to the same component.
    unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item>;

    #[inline]
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Iterates the entities carrying every term, yielding each term's component.
pub fn join<'a, J: Joinable<'a>>(terms: J) -> Join<'a, J> {
    Join {
        entities: terms.entities(),
        terms,
    }
}

pub struct Join<'a, J> {
    entities: Entities<'a>,
    terms: J,
}

impl<'a, J: Joinable<'a>> Iterator for Join<'a, J> {
    type Item = (Entity, J::Item);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let entity = self.entities.next()?;
            if let Some(item) = unsafe { self.terms.get(entity) } {
                return Some((entity, item));
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, self.entities.size_hint().1)
    }
}

macro_rules! impl_joinable {
    ($head:ident $head_term:ident $(, $tail:ident $tail_term:ident)*) => {
        unsafe impl<'a, $head: Joinable<'a> $(, $tail: Joinable<'a>)*> Joinable<'a> for ($head, $($tail,)*) {
            type Item = ($head::Item, $($tail::Item,)*);

            fn len(&self) -> usize {
                let ($head_term, $($tail_term,)*) = self;
                let len = $head_term.len();
                $(let len = len.min($tail_term.len());)*
                len
            }

            // The single-term case never reads or reassigns `len`.
            #[allow(unused_mut, unused_assignments, unused_variables)]
            fn entities(&self) -> Entities<'a> {
                let ($head_term, $($tail_term,)*) = self;
                let mut len = $head_term.len();
                let mut entities = $head_term.entities();
                $(
                    if $tail_term.len() < len {
                        len = $tail_term.len();
                        entities = $tail_term.entities();
                    }
                )*
                entities
            }

            unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
                let ($head_term, $($tail_term,)*) = self;
                unsafe {
                    Some(($head_term.get(entity)?, $($tail_term.get(entity)?,)*))
                }
            }
        }
    };
}

impl_joinable!(J0 j0);
impl_joinable!(J0 j0, J1 j1);
impl_joinable!(J0 j0, J1 j1, J2 j2);
impl_joinable!(J0 j0, J1 j1, J2 j2, J3 j3);
impl_joinable!(J0 j0, J1 j1, J2 j2, J3 j3, J4 j4);
impl_joinable!(J0 j0, J1 j1, J2 j2, J3 j3, J4 j4, J5 j5);
impl_joinable!(J0 j0, J1 j1, J2 j2, J3 j3, J4 j4, J5 j5, J6 j6);
impl_joinable!(J0 j0, J1 j1, J2 j2, J3 j3, J4 j4, J5 j5, J6 j6, J7 j7);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::Component;
    use crate::ecs::entity::{EntityGeneration, EntityIndex};
    use crate::ecs::storage::sparse_set::SparseSet;

    #[derive(Component, Debug, PartialEq)]
    struct Alpha(i32);

    #[derive(Component, Debug, PartialEq)]
    struct Beta(i32);

    #[derive(Component, Debug, PartialEq)]
    struct Gamma(i32);

    fn entity(index: u32) -> Entity {
        Entity::new(
            EntityIndex::from_bits(index),
            EntityGeneration::from_bits(1),
        )
    }

    #[test]
    fn join_pair() {
        let mut alphas = SparseSet::default();
        let mut betas = SparseSet::default();

        for index in 0..6 {
            alphas.insert(entity(index), Alpha(index as i32));
        }
        for index in [1, 3, 5, 7] {
            betas.insert(entity(index), Beta(index as i32 * 10));
        }

        let joined = join((&alphas, &betas)).collect::<Vec<_>>();
        assert_eq!(
            joined,
            vec![
                (entity(1), (&Alpha(1), &Beta(10))),
                (entity(3), (&Alpha(3), &Beta(30))),
                (entity(5), (&Alpha(5), &Beta(50))),
            ]
        );
    }

    #[test]
    fn join_drives_shortest_term() {
        let mut alphas = SparseSet::default();
        let mut betas = SparseSet::default();

        for index in 0..1000 {
            alphas.insert(entity(index), Alpha(index as i32));
        }
        betas.insert(entity(700), Beta(7));

        // Driving from `alphas` would walk 1000 entities; the shortest term
        // has one, so the join iterates exactly one candidate.
        assert_eq!(Joinable::len(&(&alphas, &betas)), 1);
        assert_eq!((&alphas, &betas).entities().count(), 1);

        let joined = join((&alphas, &betas)).collect::<Vec<_>>();
        assert_eq!(joined, vec![(entity(700), (&Alpha(700), &Beta(7)))]);
    }

    #[test]
    fn join_triple() {
        let mut alphas = SparseSet::default();
        let mut betas = SparseSet::default();
        let mut gammas = SparseSet::default();

        for index in 0..8 {
            alphas.insert(entity(index), Alpha(index as i32));
        }
        for index in [0, 2, 4, 6] {
            betas.insert(entity(index), Beta(index as i32));
        }
        for index in [2, 3, 6] {
            gammas.insert(entity(index), Gamma(index as i32));
        }

        let entities = join((&alphas, &betas, &gammas))
            .map(|(entity, _)| entity)
            .collect::<Vec<_>>();
        assert_eq!(entities, vec![entity(2), entity(6)]);
    }

    #[test]
    fn join_disjoint_is_empty() {
        let mut alphas = SparseSet::default();
        let mut betas = SparseSet::default();

        alphas.insert(entity(1), Alpha(1));
        betas.insert(entity(2), Beta(2));

        assert_eq!(join((&alphas, &betas)).count(), 0);
    }

    #[test]
    fn join_single_term() {
        let mut alphas = SparseSet::default();
        alphas.insert(entity(4), Alpha(4));

        let joined = join((&alphas,)).collect::<Vec<_>>();
        assert_eq!(joined, vec![(entity(4), (&Alpha(4),))]);
    }

    #[test]
    fn join_mut_term() {
        let mut alphas = SparseSet::default();
        let mut betas = SparseSet::default();

        for index in 0..4 {
            alphas.insert(entity(index), Alpha(index as i32));
        }
        for index in [1, 3] {
            betas.insert(entity(index), Beta(index as i32));
        }

        for (_, (alpha, beta)) in join((&mut alphas, &betas)) {
            alpha.0 += beta.0 * 100;
        }

        assert_eq!(alphas.get(entity(0)), Some(&Alpha(0)));
        assert_eq!(alphas.get(entity(1)), Some(&Alpha(101)));
        assert_eq!(alphas.get(entity(2)), Some(&Alpha(2)));
        assert_eq!(alphas.get(entity(3)), Some(&Alpha(303)));
    }

    #[test]
    fn join_mut_pair() {
        let mut alphas = SparseSet::default();
        let mut betas = SparseSet::default();

        for index in 0..3 {
            alphas.insert(entity(index), Alpha(1));
            betas.insert(entity(index), Beta(2));
        }

        for (_, (alpha, beta)) in join((&mut alphas, &mut betas)) {
            std::mem::swap(&mut alpha.0, &mut beta.0);
        }

        for index in 0..3 {
            assert_eq!(alphas.get(entity(index)), Some(&Alpha(2)));
            assert_eq!(betas.get(entity(index)), Some(&Beta(1)));
        }
    }

    #[test]
    fn join_mut_items_coexist() {
        let mut alphas = SparseSet::default();
        let betas = {
            let mut betas = SparseSet::default();
            for index in 0..4 {
                betas.insert(entity(index), Beta(0));
            }
            betas
        };

        for index in 0..4 {
            alphas.insert(entity(index), Alpha(index as i32));
        }

        // All mutable items held alive at once - distinct components, so no
        // aliasing.
        let mut items = join((&mut alphas, &betas))
            .map(|(_, (alpha, _))| alpha)
            .collect::<Vec<_>>();
        for alpha in &mut items {
            alpha.0 *= 2;
        }

        assert_eq!(alphas.get(entity(3)), Some(&Alpha(6)));
    }

    #[test]
    fn join_skips_removed_entities() {
        let mut alphas = SparseSet::default();
        let mut betas = SparseSet::default();

        for index in 0..4 {
            alphas.insert(entity(index), Alpha(index as i32));
            betas.insert(entity(index), Beta(index as i32));
        }
        betas.remove(entity(2));

        let entities = join((&alphas, &betas))
            .map(|(entity, _)| entity)
            .collect::<Vec<_>>();
        assert_eq!(entities.len(), 3);
        assert!(!entities.contains(&entity(2)));
    }
}
