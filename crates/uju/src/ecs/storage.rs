mod bits;
pub mod join;
pub mod patch;
pub mod replace;
mod section;
pub mod sparse_array;
pub mod sparse_bitset;
pub mod sparse_set;
pub mod table;
pub mod tag;
pub mod tracked;
pub mod view;

use crate::ecs::component::Component;
use crate::ecs::entity::Entity;
use crate::ecs::storage::join::Joinable;
use crate::ecs::storage::table::Table;

/// A component's table, picked by [`Component::Storage`].
///
/// Shared access always yields `&C`. Mutable access yields the storage's
/// [`Mut`](Storage::Mut) item, which a storage that tracks changes makes a
/// wrapper rather than `&mut C`. [`View`](view::View) and
/// [`ViewMut`](view::ViewMut) deref to the storage, so its own methods are
/// what a view offers, and query terms go through this trait.
pub trait Storage: Table + Default + 'static {
    type Component: Component;

    /// Whether the component travels, so that a write to a replica would be
    /// lost. Mutable access through a [`Mut`](crate::ecs::query::Mut) term or
    /// a [`ViewMut`](view::ViewMut) then skips replicas.
    const REPLICATED: bool = false;

    /// One entity's value, borrowed mutably.
    type Mut<'a>
    where
        Self: 'a;

    /// The mutable join term, built once per join from `&'a mut self`.
    type JoinMut<'a>: Joinable<'a, Item = Self::Mut<'a>>
    where
        Self: 'a;

    /// The entities holding a value, in dense order.
    fn entities(&self) -> &[Entity];

    fn get(&self, entity: Entity) -> Option<&Self::Component>;

    fn get_mut(&mut self, entity: Entity) -> Option<Self::Mut<'_>>;

    /// Inserts or replaces `entity`'s value, returning the previous one.
    fn insert(&mut self, entity: Entity, component: Self::Component) -> Option<Self::Component>;

    /// Removes `entity`'s value, returning it.
    fn remove(&mut self, entity: Entity) -> Option<Self::Component>;

    fn join_mut(&mut self) -> Self::JoinMut<'_>;
}

/// A storage that tracks the tick's changes to its entries, which the
/// [`Changed`](crate::ecs::query::Changed) filter reads.
pub trait Tracking: Storage {
    /// Whether `entity`'s value was added or marked dirty this tick.
    fn is_changed(&self, entity: Entity) -> bool;

    /// Whether `entity` gained its value this tick.
    fn is_added(&self, entity: Entity) -> bool;

    /// The entities that lost their value this tick, in removal order, as
    /// [`Table::removed`].
    fn removed(&self) -> &[Entity];
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::cell::{Cell, UnsafeCell};
    use std::ops::{Deref, DerefMut};

    use linkme::distributed_slice;

    use super::*;
    use crate::ecs::component::{COMPONENTS, Id, Registration, new_table};
    use crate::ecs::query::{Mut, Query};
    use crate::ecs::storage::join::{Entities, join};
    use crate::ecs::storage::sparse_set::SparseSet;
    use crate::ecs::world::World;

    /// A storage whose mutable item counts writes, standing in for a
    /// storage that tracks changes.
    struct Counted<T: Component> {
        set: SparseSet<T>,
        writes: u32,
    }

    impl<T: Component> Default for Counted<T> {
        fn default() -> Self {
            Self {
                set: SparseSet::new(),
                writes: 0,
            }
        }
    }

    struct CountedMut<'a, T> {
        value: &'a mut T,
        writes: &'a Cell<u32>,
    }

    impl<T> Deref for CountedMut<'_, T> {
        type Target = T;

        fn deref(&self) -> &T {
            self.value
        }
    }

    impl<T> DerefMut for CountedMut<'_, T> {
        fn deref_mut(&mut self) -> &mut T {
            self.writes.set(self.writes.get() + 1);
            self.value
        }
    }

    struct CountedJoin<'a, T: Component> {
        set: &'a mut SparseSet<T>,
        writes: &'a Cell<u32>,
    }

    // SAFETY: delegates to the `&mut SparseSet` term's guarantees.
    unsafe impl<'a, T: Component> Joinable<'a> for CountedJoin<'a, T> {
        type Item = CountedMut<'a, T>;

        fn len(&self) -> usize {
            self.set.len()
        }

        fn entities(&self) -> Entities<'a> {
            Joinable::entities(&self.set)
        }

        unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
            let value = unsafe { Joinable::get(&mut self.set, entity)? };
            Some(CountedMut {
                value,
                writes: self.writes,
            })
        }
    }

    impl<T: Component> Table for Counted<T> {
        fn as_any(&self) -> &dyn Any {
            self
        }

        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }

        fn len(&self) -> usize {
            self.set.len()
        }

        fn is_empty(&self) -> bool {
            self.set.is_empty()
        }

        fn remove(&mut self, entity: Entity) -> bool {
            self.set.remove(entity).is_some()
        }

        fn contains(&self, entity: Entity) -> bool {
            self.set.contains(entity)
        }

        fn clear(&mut self) {
            self.set.clear();
        }
    }

    impl<T: Component> Storage for Counted<T> {
        type Component = T;
        type Mut<'a> = CountedMut<'a, T>;
        type JoinMut<'a> = CountedJoin<'a, T>;

        fn entities(&self) -> &[Entity] {
            self.set.entities()
        }

        fn get(&self, entity: Entity) -> Option<&T> {
            self.set.get(entity)
        }

        fn get_mut(&mut self, entity: Entity) -> Option<CountedMut<'_, T>> {
            Some(CountedMut {
                value: self.set.get_mut(entity)?,
                writes: Cell::from_mut(&mut self.writes),
            })
        }

        fn insert(&mut self, entity: Entity, component: T) -> Option<T> {
            self.set.insert(entity, component)
        }

        fn remove(&mut self, entity: Entity) -> Option<T> {
            self.set.remove(entity)
        }

        fn join_mut(&mut self) -> CountedJoin<'_, T> {
            CountedJoin {
                set: &mut self.set,
                writes: Cell::from_mut(&mut self.writes),
            }
        }
    }

    #[derive(Debug, PartialEq)]
    struct Score(i32);

    #[distributed_slice(COMPONENTS)]
    static SCORE: Registration = Registration {
        name: concat!(module_path!(), "::Score"),
        id: UnsafeCell::new(u16::MAX),
        new_table: new_table::<Score>,
    };

    impl Component for Score {
        type Storage = Counted<Score>;

        fn id() -> Id {
            unsafe { *SCORE.id.get() }
        }
    }

    #[derive(crate::ecs::Component)]
    struct Bonus(i32);

    #[test]
    fn storage_picks_mut_item() {
        crate::ecs::init();
        let world = World::new();
        let entities = (0..4).map(|_| world.spawn()).collect::<Vec<_>>();

        for (index, &entity) in entities.iter().enumerate() {
            world.view_mut::<Score>().set.insert(entity, Score(0));
            if index % 2 == 1 {
                world
                    .view_mut::<Bonus>()
                    .insert(entity, Bonus(index as i32));
            }
        }

        // reading through the mutable item writes nothing
        let mut query = Query::<Mut<Score>>::new(&world);
        let total = query.iter().map(|(_, score)| score.0).sum::<i32>();
        assert_eq!(total, 0);
        drop(query);
        assert_eq!(world.view::<Score>().writes, 0);

        let mut query = Query::<(Mut<Score>, Bonus)>::new(&world);
        for (_, (mut score, bonus)) in query.iter() {
            score.0 += bonus.0;
        }
        drop(query);
        assert_eq!(world.view::<Score>().writes, 2);

        {
            let mut scores = world.view_mut::<Score>();
            let bonuses = world.view::<Bonus>();
            for (_, (mut score, bonus)) in join((&mut scores, &bonuses)) {
                score.0 *= bonus.0;
            }
            scores.get_mut(entities[0]).unwrap().0 = 7;
        }

        let scores = world.view::<Score>();
        assert_eq!(scores.writes, 5);
        assert_eq!(scores.get(entities[0]), Some(&Score(7)));
        assert_eq!(scores.get(entities[1]), Some(&Score(1)));
        assert_eq!(scores.get(entities[2]), Some(&Score(0)));
        assert_eq!(scores.get(entities[3]), Some(&Score(9)));
    }
}
