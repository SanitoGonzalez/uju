use std::marker::PhantomData;

use crate::ecs::{
    component::Component,
    entity::Entity,
    storage::{
        join::{Entities, Joinable, join},
        sparse_set::SparseSet,
        view::{View, ViewMut},
    },
    world::World,
};

/// Marks a query term as mutable: `tx.query::<(Mut<Position>, Velocity)>()`
/// yields `&mut Position` alongside `&Velocity`.
pub struct Mut<C: Component>(PhantomData<C>);

/// Filters a query to entities carrying `C`, contributing nothing to the
/// yielded item.
///
/// Holds a shared borrow of `C`'s table, so combining it with `Mut<C>` over
/// the same component panics (and is redundant - `Mut<C>` already requires
/// presence). When `C`'s table is the smallest term, it drives iteration.
pub struct With<C: Component>(PhantomData<C>);

/// Filters a query to entities not carrying `C`, contributing nothing to the
/// yielded item.
///
/// Holds a shared borrow of `C`'s table. Its entity set is a complement, so
/// it can never drive iteration - a query needs at least one non-`Without`
/// term.
pub struct Without<C: Component>(PhantomData<C>);

/// A term of a [`Query`]: a bare component type reads (`&C`), [`Mut<C>`]
/// writes (`&mut C`), [`With<C>`]/[`Without<C>`] filter, and a tuple of terms
/// composes. Filters are stripped from the yielded item, and tuples flatten:
/// `(A, B, (With<Foo>, Without<Bar>))` yields `(&A, &B)`, and a lone data
/// term yields its item bare - `(Mut<A>, With<Foo>)` yields `&mut A`.
pub trait Term {
    /// The storage borrow this term checks out of the [`World`].
    type View<'w>;
    /// [`View`](Term::View) as a join term, re-borrowed for one iteration `'q`.
    type Join<'q, 'w: 'q>: Joinable<'q>;
    /// This term's contribution folded onto the already-collected `Rest`:
    /// [`Cons`] of the component borrow for data terms, `Rest` unchanged for
    /// filters.
    type Item<'q, 'w: 'q, Rest>;

    fn fetch(world: &World) -> Self::View<'_>;
    fn join<'q, 'w>(view: &'q mut Self::View<'w>) -> Self::Join<'q, 'w>;
    fn item<'q, 'w: 'q, Rest>(
        item: <Self::Join<'q, 'w> as Joinable<'q>>::Item,
        rest: Rest,
    ) -> Self::Item<'q, 'w, Rest>;
}

impl<C: Component> Term for C {
    type View<'w> = View<'w, C>;
    type Join<'q, 'w: 'q> = &'q View<'w, C>;
    type Item<'q, 'w: 'q, Rest> = Cons<&'q C, Rest>;

    fn fetch(world: &World) -> Self::View<'_> {
        world.view()
    }

    fn join<'q, 'w>(view: &'q mut Self::View<'w>) -> Self::Join<'q, 'w> {
        view
    }

    #[inline]
    fn item<'q, 'w: 'q, Rest>(item: &'q C, rest: Rest) -> Self::Item<'q, 'w, Rest> {
        Cons(item, rest)
    }
}

impl<C: Component> Term for Mut<C> {
    type View<'w> = ViewMut<'w, C>;
    type Join<'q, 'w: 'q> = &'q mut ViewMut<'w, C>;
    type Item<'q, 'w: 'q, Rest> = Cons<&'q mut C, Rest>;

    fn fetch(world: &World) -> Self::View<'_> {
        world.view_mut()
    }

    fn join<'q, 'w>(view: &'q mut Self::View<'w>) -> Self::Join<'q, 'w> {
        view
    }

    #[inline]
    fn item<'q, 'w: 'q, Rest>(item: &'q mut C, rest: Rest) -> Self::Item<'q, 'w, Rest> {
        Cons(item, rest)
    }
}

impl<C: Component> Term for With<C> {
    type View<'w> = View<'w, C>;
    type Join<'q, 'w: 'q> = WithJoin<'q, C>;
    type Item<'q, 'w: 'q, Rest> = Rest;

    fn fetch(world: &World) -> Self::View<'_> {
        world.view()
    }

    fn join<'q, 'w>(view: &'q mut Self::View<'w>) -> Self::Join<'q, 'w> {
        WithJoin(view)
    }

    #[inline]
    fn item<'q, 'w: 'q, Rest>((): (), rest: Rest) -> Self::Item<'q, 'w, Rest> {
        rest
    }
}

impl<C: Component> Term for Without<C> {
    type View<'w> = View<'w, C>;
    type Join<'q, 'w: 'q> = WithoutJoin<'q, C>;
    type Item<'q, 'w: 'q, Rest> = Rest;

    fn fetch(world: &World) -> Self::View<'_> {
        world.view()
    }

    fn join<'q, 'w>(view: &'q mut Self::View<'w>) -> Self::Join<'q, 'w> {
        WithoutJoin(view)
    }

    #[inline]
    fn item<'q, 'w: 'q, Rest>((): (), rest: Rest) -> Self::Item<'q, 'w, Rest> {
        rest
    }
}

/// [`With`] as a join term.
pub struct WithJoin<'q, C: Component>(&'q SparseSet<C>);

// SAFETY: `entities` delegates to the shared `SparseSet` term's guarantees;
// `get` hands out no references, so at-most-once is trivially upheld.
unsafe impl<'q, C: Component> Joinable<'q> for WithJoin<'q, C> {
    type Item = ();

    #[inline]
    fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    fn entities(&self) -> Entities<'q> {
        Joinable::entities(&self.0)
    }

    #[inline]
    unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
        self.0.contains(entity).then_some(())
    }
}

/// [`Without`] as a join term.
pub struct WithoutJoin<'q, C: Component>(&'q SparseSet<C>);

// SAFETY: `entities` is unreachable (`len` keeps this term from ever being
// the shortest); `get` hands out no references.
unsafe impl<'q, C: Component> Joinable<'q> for WithoutJoin<'q, C> {
    type Item = ();

    #[inline]
    fn len(&self) -> usize {
        // the set lists the excluded entities - iterating it would yield the
        // complement's complement, so this term must never drive the join
        usize::MAX
    }

    fn entities(&self) -> Entities<'q> {
        panic!("`Without` cannot drive iteration - a query needs at least one non-`Without` term")
    }

    #[inline]
    unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
        (!self.0.contains(entity)).then_some(())
    }
}

/// One collected query item prepended onto the rest, built by
/// [`Term::item`] and turned back into a flat tuple by [`Flatten`].
pub struct Cons<Head, Rest>(Head, Rest);

/// The empty end of a [`Cons`] chain.
pub struct Nil;

/// Flattens the [`Cons`] chain a term fold produces into the tuple a query
/// yields. A single collected item flattens to itself, unwrapped.
pub trait Flatten {
    type Flat;

    fn flatten(self) -> Self::Flat;
}

impl Flatten for Nil {
    type Flat = ();

    #[inline]
    fn flatten(self) -> Self::Flat {}
}

impl<Head> Flatten for Cons<Head, Nil> {
    type Flat = Head;

    #[inline]
    fn flatten(self) -> Self::Flat {
        self.0
    }
}

macro_rules! cons_ty {
    () => { Nil };
    ($H:ident $(, $T:ident)*) => { Cons<$H, cons_ty!($($T),*)> };
}

macro_rules! cons_pat {
    () => { Nil };
    ($h:ident $(, $t:ident)*) => { Cons($h, cons_pat!($($t),*)) };
}

macro_rules! impl_flatten {
    ($($T:ident $t:ident),+) => {
        impl<$($T),+> Flatten for cons_ty!($($T),+) {
            type Flat = ($($T,)+);

            #[inline]
            fn flatten(self) -> Self::Flat {
                let cons_pat!($($t),+) = self;
                ($($t,)+)
            }
        }
    };
}

impl_flatten!(T0 t0, T1 t1);
impl_flatten!(T0 t0, T1 t1, T2 t2);
impl_flatten!(T0 t0, T1 t1, T2 t2, T3 t3);
impl_flatten!(T0 t0, T1 t1, T2 t2, T3 t3, T4 t4);
impl_flatten!(T0 t0, T1 t1, T2 t2, T3 t3, T4 t4, T5 t5);
impl_flatten!(T0 t0, T1 t1, T2 t2, T3 t3, T4 t4, T5 t5, T6 t6);
impl_flatten!(T0 t0, T1 t1, T2 t2, T3 t3, T4 t4, T5 t5, T6 t6, T7 t7);
impl_flatten!(T0 t0, T1 t1, T2 t2, T3 t3, T4 t4, T5 t5, T6 t6, T7 t7, T8 t8);
impl_flatten!(T0 t0, T1 t1, T2 t2, T3 t3, T4 t4, T5 t5, T6 t6, T7 t7, T8 t8, T9 t9);
impl_flatten!(T0 t0, T1 t1, T2 t2, T3 t3, T4 t4, T5 t5, T6 t6, T7 t7, T8 t8, T9 t9, T10 t10);
impl_flatten!(
    T0 t0, T1 t1, T2 t2, T3 t3, T4 t4, T5 t5, T6 t6, T7 t7, T8 t8, T9 t9, T10 t10, T11 t11
);

macro_rules! fold_item_ty {
    ($Rest:ty; $q:lifetime, $w:lifetime;) => { $Rest };
    ($Rest:ty; $q:lifetime, $w:lifetime; $T:ident $(, $Ts:ident)*) => {
        <$T as Term>::Item<$q, $w, fold_item_ty!($Rest; $q, $w; $($Ts),*)>
    };
}

macro_rules! fold_item_expr {
    ($rest:expr;) => { $rest };
    ($rest:expr; $T:ident $t:ident $(, $Ts:ident $ts:ident)*) => {
        <$T as Term>::item($t, fold_item_expr!($rest; $($Ts $ts),*))
    };
}

macro_rules! impl_term {
    ($($T:ident $t:ident),+) => {
        impl<$($T: Term),+> Term for ($($T,)+) {
            type View<'w> = ($($T::View<'w>,)+);
            type Join<'q, 'w: 'q> = ($($T::Join<'q, 'w>,)+);
            type Item<'q, 'w: 'q, Rest> = fold_item_ty!(Rest; 'q, 'w; $($T),+);

            fn fetch(world: &World) -> Self::View<'_> {
                ($($T::fetch(world),)+)
            }

            fn join<'q, 'w>(view: &'q mut Self::View<'w>) -> Self::Join<'q, 'w> {
                let ($($t,)+) = view;
                ($($T::join($t),)+)
            }

            #[inline]
            fn item<'q, 'w: 'q, Rest>(
                item: <Self::Join<'q, 'w> as Joinable<'q>>::Item,
                rest: Rest,
            ) -> Self::Item<'q, 'w, Rest> {
                let ($($t,)+) = item;
                fold_item_expr!(rest; $($T $t),+)
            }
        }
    };
}

impl_term!(T0 t0, T1 t1);
impl_term!(T0 t0, T1 t1, T2 t2);
impl_term!(T0 t0, T1 t1, T2 t2, T3 t3);
impl_term!(T0 t0, T1 t1, T2 t2, T3 t3, T4 t4);
impl_term!(T0 t0, T1 t1, T2 t2, T3 t3, T4 t4, T5 t5);
impl_term!(T0 t0, T1 t1, T2 t2, T3 t3, T4 t4, T5 t5, T6 t6);
impl_term!(T0 t0, T1 t1, T2 t2, T3 t3, T4 t4, T5 t5, T6 t6, T7 t7);
impl_term!(T0 t0, T1 t1, T2 t2, T3 t3, T4 t4, T5 t5, T6 t6, T7 t7, T8 t8);
impl_term!(T0 t0, T1 t1, T2 t2, T3 t3, T4 t4, T5 t5, T6 t6, T7 t7, T8 t8, T9 t9);
impl_term!(T0 t0, T1 t1, T2 t2, T3 t3, T4 t4, T5 t5, T6 t6, T7 t7, T8 t8, T9 t9, T10 t10);
impl_term!(
    T0 t0, T1 t1, T2 t2, T3 t3, T4 t4, T5 t5, T6 t6, T7 t7, T8 t8, T9 t9, T10 t10, T11 t11
);

/// A batch query checked out of the [`World`]: one [`View`]/[`ViewMut`] per
/// term, held for the query's whole lifetime.
///
/// Borrows are dynamic (`RefCell`); a term conflicting with any live borrow
/// of the same component (another query, a point-access guard) panics at
/// [`Tx::query`](crate::ecs::tx::Tx::query).
pub struct Query<'w, Q: Term> {
    views: Q::View<'w>,
}

impl<'w, Q: Term> Query<'w, Q> {
    pub(crate) fn new(world: &'w World) -> Self {
        Self {
            views: Q::fetch(world),
        }
    }

    /// Iterates the entities matching every term, yielding the data terms'
    /// components as a flat tuple - filters are stripped, and a lone data
    /// term yields its component bare.
    pub fn iter<'q>(
        &'q mut self,
    ) -> impl Iterator<Item = (Entity, <Q::Item<'q, 'w, Nil> as Flatten>::Flat)>
    where
        Q::Item<'q, 'w, Nil>: Flatten,
    {
        join(Q::join(&mut self.views)).map(|(entity, item)| (entity, Q::item(item, Nil).flatten()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::Component;
    use crate::ecs::entity::{EntityGeneration, EntityIndex};

    #[derive(Component, Debug, PartialEq)]
    struct Position(i32);

    #[derive(Component, Debug, PartialEq)]
    struct Velocity(i32);

    #[derive(Component, Debug, PartialEq)]
    struct Frozen;

    fn entity(index: u32) -> Entity {
        Entity::new(
            EntityIndex::from_bits(index),
            EntityGeneration::from_bits(1),
        )
    }

    #[test]
    fn query_terms() {
        crate::ecs::init();
        let world = World::new();

        for index in 0..3 {
            world
                .view_mut::<Position>()
                .insert(entity(index), Position(0));
            world
                .view_mut::<Velocity>()
                .insert(entity(index), Velocity(index as i32));
        }
        world.view_mut::<Frozen>().insert(entity(1), Frozen);

        // mixed mutability, three terms - only the frozen entity matches all
        let mut query = Query::<(Mut<Position>, Velocity, Frozen)>::new(&world);
        for (_, (position, velocity, _)) in query.iter() {
            position.0 -= velocity.0;
        }
        drop(query);

        // single bare term reads
        let mut query = Query::<Position>::new(&world);
        let positions = query.iter().map(|(e, p)| (e, p.0)).collect::<Vec<_>>();
        assert_eq!(
            positions,
            vec![(entity(0), 0), (entity(1), -1), (entity(2), 0)]
        );
    }

    #[test]
    fn query_filters() {
        crate::ecs::init();
        let world = World::new();

        for index in 0..4 {
            world
                .view_mut::<Position>()
                .insert(entity(index), Position(0));
        }
        for index in [1, 2] {
            world
                .view_mut::<Velocity>()
                .insert(entity(index), Velocity(7));
        }
        world.view_mut::<Frozen>().insert(entity(2), Frozen);

        // filters are stripped from the item; the lone data term yields bare
        let mut query = Query::<(Mut<Position>, (With<Velocity>, Without<Frozen>))>::new(&world);
        for (e, position) in query.iter() {
            assert_eq!(e, entity(1));
            position.0 = 10;
        }
        drop(query);

        // flat filters strip the same way, leaving the data tuple
        let mut query = Query::<(Position, Velocity, Without<Frozen>)>::new(&world);
        let matched = query
            .iter()
            .map(|(e, (position, velocity))| (e, position.0, velocity.0))
            .collect::<Vec<_>>();
        assert_eq!(matched, vec![(entity(1), 10, 7)]);
    }

    #[test]
    fn query_with_drives() {
        crate::ecs::init();
        let world = World::new();

        for index in 0..3 {
            world
                .view_mut::<Position>()
                .insert(entity(index), Position(0));
        }
        world.view_mut::<Frozen>().insert(entity(2), Frozen);

        // `Frozen` is the smallest table, so `With` drives the iteration
        let mut query = Query::<(Mut<Position>, With<Frozen>)>::new(&world);
        let matched = query
            .iter()
            .map(|(e, position)| (e, position.0))
            .collect::<Vec<_>>();
        assert_eq!(matched, vec![(entity(2), 0)]);
    }

    #[derive(Component, Debug, PartialEq)]
    struct C0(i32);
    #[derive(Component, Debug, PartialEq)]
    struct C1(i32);
    #[derive(Component, Debug, PartialEq)]
    struct C2(i32);
    #[derive(Component, Debug, PartialEq)]
    struct C3(i32);
    #[derive(Component, Debug, PartialEq)]
    struct C4(i32);
    #[derive(Component, Debug, PartialEq)]
    struct C5(i32);
    #[derive(Component, Debug, PartialEq)]
    struct C6(i32);
    #[derive(Component, Debug, PartialEq)]
    struct C7(i32);
    #[derive(Component, Debug, PartialEq)]
    struct C8(i32);
    #[derive(Component, Debug, PartialEq)]
    struct C9(i32);
    #[derive(Component, Debug, PartialEq)]
    struct C10(i32);
    #[derive(Component, Debug, PartialEq)]
    struct C11(i32);

    #[test]
    fn query_max_arity() {
        crate::ecs::init();
        let world = World::new();

        let target = entity(0);
        world.view_mut::<C0>().insert(target, C0(0));
        world.view_mut::<C1>().insert(target, C1(1));
        world.view_mut::<C2>().insert(target, C2(2));
        world.view_mut::<C3>().insert(target, C3(3));
        world.view_mut::<C4>().insert(target, C4(4));
        world.view_mut::<C5>().insert(target, C5(5));
        world.view_mut::<C6>().insert(target, C6(6));
        world.view_mut::<C7>().insert(target, C7(7));
        world.view_mut::<C8>().insert(target, C8(8));
        world.view_mut::<C9>().insert(target, C9(9));
        world.view_mut::<C10>().insert(target, C10(10));

        // 12 terms: 11 data (one mutable) + a filter that strips away
        let mut query = Query::<(
            Mut<C0>,
            C1,
            C2,
            C3,
            C4,
            C5,
            C6,
            C7,
            C8,
            C9,
            C10,
            Without<C11>,
        )>::new(&world);

        let matched = query
            .iter()
            .map(|(entity, (c0, c1, c2, c3, c4, c5, c6, c7, c8, c9, c10))| {
                c0.0 = 100;
                (
                    entity,
                    c1.0 + c2.0 + c3.0 + c4.0 + c5.0 + c6.0 + c7.0 + c8.0 + c9.0 + c10.0,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(matched, vec![(target, 55)]);
        drop(query);

        assert_eq!(world.view::<C0>().get(target), Some(&C0(100)));
    }

    #[test]
    #[should_panic(expected = "cannot drive iteration")]
    fn query_without_alone_panics() {
        crate::ecs::init();
        let world = World::new();

        let _ = Query::<Without<Frozen>>::new(&world).iter();
    }
}
