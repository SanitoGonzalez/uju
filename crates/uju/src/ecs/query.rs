use std::cell::Ref;
use std::marker::PhantomData;

use crate::ecs::{
    cell::{Cell, CellIndex},
    component::Component,
    entity::{Entity, Replicas},
    storage::{
        Storage, Tracking,
        join::{Entities, Join, Joinable},
        table::Table,
        view::{View, ViewMut},
    },
    world::World,
};

/// Marks a query term as mutable: `Query::<(Mut<Position>, Velocity)>`
/// yields `Position`'s [`Storage::Mut`] item alongside `&Velocity`. That is
/// `&mut Position` when `Position` is stored in a
/// [`SparseSet`](crate::ecs::storage::sparse_set::SparseSet).
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
/// it never drives iteration.
pub struct Without<C: Component>(PhantomData<C>);

/// Filters a query to entities whose `C` was added or marked dirty since the
/// tick began, contributing nothing to the yielded item. An added value is
/// marked dirty too, so it matches both `Changed<C>` and [`Added<C>`].
///
/// `C`'s storage must track changes, such as a
/// [`Tracked`](crate::ecs::storage::tracked::Tracked) one:
///
/// ```compile_fail,E0277
/// # use uju::prelude::*;
/// #[derive(Component)]
/// struct Hp(u32);
///
/// let world = World::new();
/// let _ = Query::<(Hp, Changed<Hp>)>::new(&world); // a `SparseSet`
/// ```
///
/// Holds a shared borrow of `C`'s table, and drives like [`With<C>`].
pub struct Changed<C: Component>(PhantomData<C>);

/// Filters a query to entities that gained `C` since the tick began,
/// contributing nothing to the yielded item.
///
/// `C`'s storage must track changes, such as a
/// [`Tracked`](crate::ecs::storage::tracked::Tracked) one:
///
/// ```compile_fail,E0277
/// # use uju::prelude::*;
/// #[derive(Component)]
/// struct Hp(u32);
///
/// let world = World::new();
/// let _ = Query::<(Hp, Added<Hp>)>::new(&world); // a `SparseSet`
/// ```
///
/// Holds a shared borrow of `C`'s table, and drives like [`With<C>`].
pub struct Added<C: Component>(PhantomData<C>);

/// Yields whether the entity carries `C`, as a `bool`, matching every entity.
///
/// Holds a shared borrow of `C`'s table. It never drives iteration.
pub struct Has<C: Component>(PhantomData<C>);

/// Filters a query to entities matching any of the [`Filter`]s in the tuple
/// `T`, such as `Or<(With<A>, Changed<B>)>`, contributing nothing to the
/// yielded item. A nested tuple matches when all of its filters do.
///
/// Holds its filters' borrows. It never drives iteration.
pub struct Or<T>(PhantomData<T>);

/// Filters a query to the entities this world owns, skipping replicas.
///
/// Holds a shared borrow of the [`Replicas`] flags. It never drives
/// iteration.
pub struct Owned;

/// Filters a query to replicas, the entities another shard owns.
///
/// Holds a shared borrow of the [`Replicas`] flags. It never drives
/// iteration.
pub struct Replica;

/// A term of a [`Query`]: a bare component type reads (`&C`), [`Mut<C>`]
/// writes (the storage's [`Storage::Mut`]), `Option<C>` and
/// `Option<Mut<C>>` read or write when present, [`Has<C>`] yields a `bool`,
/// the [`Filter`]s filter, and a tuple of terms composes. Filters are
/// stripped from the yielded item, and tuples flatten:
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

    /// Whether the term writes a [replicated](Storage::REPLICATED) storage,
    /// so the query skips replicas.
    const SKIPS_REPLICAS: bool = false;

    fn fetch(world: &World) -> Self::View<'_>;
    fn join<'q, 'w>(view: &'q mut Self::View<'w>) -> Self::Join<'q, 'w>;
    fn item<'q, 'w: 'q, Rest>(
        item: <Self::Join<'q, 'w> as Joinable<'q>>::Item,
        rest: Rest,
    ) -> Self::Item<'q, 'w, Rest>;
}

/// A [`Term`] that only filters, contributing nothing to the yielded item:
/// [`With`], [`Without`], [`Changed`], [`Added`], [`Owned`], [`Replica`],
/// [`Or`], and tuples of filters. [`Or`] takes filters.
pub trait Filter: Term {}

impl<C: Component> Filter for With<C> {}
impl<C: Component> Filter for Without<C> {}
impl<C: Component<Storage: Tracking>> Filter for Changed<C> {}
impl<C: Component<Storage: Tracking>> Filter for Added<C> {}
impl Filter for Owned {}
impl Filter for Replica {}

impl<C: Component> Term for C {
    type View<'w> = View<'w, C>;
    type Join<'q, 'w: 'q> = &'q C::Storage;
    type Item<'q, 'w: 'q, Rest> = Cons<&'q C, Rest>;

    fn fetch(world: &World) -> Self::View<'_> {
        world.view()
    }

    #[inline]
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
    type Join<'q, 'w: 'q> = <C::Storage as Storage>::JoinMut<'q>;
    type Item<'q, 'w: 'q, Rest> = Cons<<C::Storage as Storage>::Mut<'q>, Rest>;

    const SKIPS_REPLICAS: bool = <C::Storage as Storage>::REPLICATED;

    fn fetch(world: &World) -> Self::View<'_> {
        world.view_mut()
    }

    #[inline]
    fn join<'q, 'w>(view: &'q mut Self::View<'w>) -> Self::Join<'q, 'w> {
        view.join_mut()
    }

    #[inline]
    fn item<'q, 'w: 'q, Rest>(
        item: <C::Storage as Storage>::Mut<'q>,
        rest: Rest,
    ) -> Self::Item<'q, 'w, Rest> {
        Cons(item, rest)
    }
}

impl<C: Component> Term for With<C> {
    type View<'w> = View<'w, C>;
    type Join<'q, 'w: 'q> = WithJoin<'q, C::Storage>;
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
    type Join<'q, 'w: 'q> = WithoutJoin<'q, C::Storage>;
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

impl<C: Component<Storage: Tracking>> Term for Changed<C> {
    type View<'w> = View<'w, C>;
    type Join<'q, 'w: 'q> = ChangedJoin<'q, C::Storage>;
    type Item<'q, 'w: 'q, Rest> = Rest;

    fn fetch(world: &World) -> Self::View<'_> {
        world.view()
    }

    fn join<'q, 'w>(view: &'q mut Self::View<'w>) -> Self::Join<'q, 'w> {
        ChangedJoin(view)
    }

    #[inline]
    fn item<'q, 'w: 'q, Rest>((): (), rest: Rest) -> Self::Item<'q, 'w, Rest> {
        rest
    }
}

/// [`Changed`] as a join term.
pub struct ChangedJoin<'q, S: Tracking>(&'q S);

// SAFETY: `entities` delegates to the shared storage term's guarantees;
// `get` hands out no references.
unsafe impl<'q, S: Tracking> Joinable<'q> for ChangedJoin<'q, S> {
    type Item = ();

    #[inline]
    fn len(&self) -> usize {
        Table::len(self.0)
    }

    #[inline]
    fn entities(&self) -> Entities<'q> {
        Joinable::entities(&self.0)
    }

    #[inline]
    unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
        self.0.is_changed(entity).then_some(())
    }
}

impl<C: Component<Storage: Tracking>> Term for Added<C> {
    type View<'w> = View<'w, C>;
    type Join<'q, 'w: 'q> = AddedJoin<'q, C::Storage>;
    type Item<'q, 'w: 'q, Rest> = Rest;

    fn fetch(world: &World) -> Self::View<'_> {
        world.view()
    }

    fn join<'q, 'w>(view: &'q mut Self::View<'w>) -> Self::Join<'q, 'w> {
        AddedJoin(view)
    }

    #[inline]
    fn item<'q, 'w: 'q, Rest>((): (), rest: Rest) -> Self::Item<'q, 'w, Rest> {
        rest
    }
}

/// [`Added`] as a join term.
pub struct AddedJoin<'q, S: Tracking>(&'q S);

// SAFETY: `entities` delegates to the shared storage term's guarantees;
// `get` hands out no references.
unsafe impl<'q, S: Tracking> Joinable<'q> for AddedJoin<'q, S> {
    type Item = ();

    #[inline]
    fn len(&self) -> usize {
        Table::len(self.0)
    }

    #[inline]
    fn entities(&self) -> Entities<'q> {
        Joinable::entities(&self.0)
    }

    #[inline]
    unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
        self.0.is_added(entity).then_some(())
    }
}

impl<C: Component> Term for Has<C> {
    type View<'w> = View<'w, C>;
    type Join<'q, 'w: 'q> = HasJoin<'q, C::Storage>;
    type Item<'q, 'w: 'q, Rest> = Cons<bool, Rest>;

    fn fetch(world: &World) -> Self::View<'_> {
        world.view()
    }

    fn join<'q, 'w>(view: &'q mut Self::View<'w>) -> Self::Join<'q, 'w> {
        HasJoin(view)
    }

    #[inline]
    fn item<'q, 'w: 'q, Rest>(item: bool, rest: Rest) -> Self::Item<'q, 'w, Rest> {
        Cons(item, rest)
    }
}

/// [`Has`] as a join term.
pub struct HasJoin<'q, S: Storage>(&'q S);

// SAFETY: `entities` is unreachable (`len` keeps this term from ever being
// the shortest); `get` hands out no references.
unsafe impl<'q, S: Storage> Joinable<'q> for HasJoin<'q, S> {
    type Item = bool;

    #[inline]
    fn len(&self) -> usize {
        usize::MAX
    }

    fn entities(&self) -> Entities<'q> {
        unreachable!("`Has` never drives")
    }

    #[inline]
    unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
        Some(Table::contains(self.0, entity))
    }
}

impl<C: Component> Term for Option<C> {
    type View<'w> = View<'w, C>;
    type Join<'q, 'w: 'q> = OptionJoin<&'q C::Storage>;
    type Item<'q, 'w: 'q, Rest> = Cons<Option<&'q C>, Rest>;

    fn fetch(world: &World) -> Self::View<'_> {
        world.view()
    }

    #[inline]
    fn join<'q, 'w>(view: &'q mut Self::View<'w>) -> Self::Join<'q, 'w> {
        OptionJoin(view)
    }

    #[inline]
    fn item<'q, 'w: 'q, Rest>(item: Option<&'q C>, rest: Rest) -> Self::Item<'q, 'w, Rest> {
        Cons(item, rest)
    }
}

impl<C: Component> Term for Option<Mut<C>> {
    type View<'w> = ViewMut<'w, C>;
    type Join<'q, 'w: 'q> = OptionJoin<<C::Storage as Storage>::JoinMut<'q>>;
    type Item<'q, 'w: 'q, Rest> = Cons<Option<<C::Storage as Storage>::Mut<'q>>, Rest>;

    // the whole query skips replicas, as for `Mut<C>`, rather than yielding
    // `None` for a replica that holds `C`
    const SKIPS_REPLICAS: bool = <C::Storage as Storage>::REPLICATED;

    fn fetch(world: &World) -> Self::View<'_> {
        world.view_mut()
    }

    #[inline]
    fn join<'q, 'w>(view: &'q mut Self::View<'w>) -> Self::Join<'q, 'w> {
        OptionJoin(view.join_mut())
    }

    #[inline]
    fn item<'q, 'w: 'q, Rest>(
        item: Option<<C::Storage as Storage>::Mut<'q>>,
        rest: Rest,
    ) -> Self::Item<'q, 'w, Rest> {
        Cons(item, rest)
    }
}

/// An `Option` term as a join term: matches every entity, and yields the
/// inner term's item when it matches.
pub struct OptionJoin<J>(J);

// SAFETY: `entities` is unreachable (`len` keeps this term from ever being
// the shortest); `get` probes the inner term once per probe of its own.
unsafe impl<'q, J: Joinable<'q>> Joinable<'q> for OptionJoin<J> {
    type Item = Option<J::Item>;

    #[inline]
    fn len(&self) -> usize {
        usize::MAX
    }

    fn entities(&self) -> Entities<'q> {
        unreachable!("`Option` never drives")
    }

    #[inline]
    unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
        Some(unsafe { self.0.get(entity) })
    }
}

/// [`With`] as a join term.
pub struct WithJoin<'q, S: Storage>(&'q S);

// SAFETY: `entities` delegates to the shared storage term's guarantees;
// `get` hands out no references, so at-most-once is trivially upheld.
unsafe impl<'q, S: Storage> Joinable<'q> for WithJoin<'q, S> {
    type Item = ();

    #[inline]
    fn len(&self) -> usize {
        Table::len(self.0)
    }

    #[inline]
    fn entities(&self) -> Entities<'q> {
        Joinable::entities(&self.0)
    }

    #[inline]
    unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
        Table::contains(self.0, entity).then_some(())
    }
}

/// [`Without`] as a join term.
pub struct WithoutJoin<'q, S: Storage>(&'q S);

// SAFETY: `entities` is unreachable (`len` keeps this term from ever being
// the shortest); `get` hands out no references.
unsafe impl<'q, S: Storage> Joinable<'q> for WithoutJoin<'q, S> {
    type Item = ();

    #[inline]
    fn len(&self) -> usize {
        // the set lists the excluded entities - iterating it would yield the
        // complement's complement, so this term must never drive the join
        usize::MAX
    }

    fn entities(&self) -> Entities<'q> {
        unreachable!("`Without` never drives")
    }

    #[inline]
    unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
        (!Table::contains(self.0, entity)).then_some(())
    }
}

impl Term for Owned {
    type View<'w> = Ref<'w, Replicas>;
    type Join<'q, 'w: 'q> = OwnedJoin<'q>;
    type Item<'q, 'w: 'q, Rest> = Rest;

    fn fetch(world: &World) -> Self::View<'_> {
        world.entities().replicas()
    }

    fn join<'q, 'w>(view: &'q mut Self::View<'w>) -> Self::Join<'q, 'w> {
        OwnedJoin(view)
    }

    #[inline]
    fn item<'q, 'w: 'q, Rest>((): (), rest: Rest) -> Self::Item<'q, 'w, Rest> {
        rest
    }
}

impl Term for Replica {
    type View<'w> = Ref<'w, Replicas>;
    type Join<'q, 'w: 'q> = ReplicaJoin<'q>;
    type Item<'q, 'w: 'q, Rest> = Rest;

    fn fetch(world: &World) -> Self::View<'_> {
        world.entities().replicas()
    }

    fn join<'q, 'w>(view: &'q mut Self::View<'w>) -> Self::Join<'q, 'w> {
        ReplicaJoin(view)
    }

    #[inline]
    fn item<'q, 'w: 'q, Rest>((): (), rest: Rest) -> Self::Item<'q, 'w, Rest> {
        rest
    }
}

/// [`Owned`] as a join term.
pub struct OwnedJoin<'q>(&'q Replicas);

// SAFETY: `entities` is unreachable (`len` keeps this term from ever being
// the shortest); `get` hands out no references.
unsafe impl<'q> Joinable<'q> for OwnedJoin<'q> {
    type Item = ();

    #[inline]
    fn len(&self) -> usize {
        usize::MAX
    }

    fn entities(&self) -> Entities<'q> {
        unreachable!("`Owned` never drives")
    }

    #[inline]
    unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
        (!self.0.contains(entity)).then_some(())
    }
}

/// The [`Owned`] filter a query adds once when a term
/// [skips replicas](Term::SKIPS_REPLICAS), so the flag is read once per
/// entity whatever the number of such terms.
struct OwnedIf<'q>(Option<&'q Replicas>);

// SAFETY: `entities` is unreachable (`len` keeps this term from ever being
// the shortest); `get` hands out no references.
unsafe impl<'q> Joinable<'q> for OwnedIf<'q> {
    type Item = ();

    #[inline]
    fn len(&self) -> usize {
        usize::MAX
    }

    fn entities(&self) -> Entities<'q> {
        unreachable!("a query's replica filter never drives")
    }

    #[inline]
    unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
        match self.0 {
            Some(replicas) => (!replicas.contains(entity)).then_some(()),
            None => Some(()),
        }
    }
}

/// [`Replica`] as a join term.
pub struct ReplicaJoin<'q>(&'q Replicas);

// SAFETY: `entities` is unreachable (`len` keeps this term from ever being
// the shortest); `get` hands out no references.
unsafe impl<'q> Joinable<'q> for ReplicaJoin<'q> {
    type Item = ();

    #[inline]
    fn len(&self) -> usize {
        usize::MAX
    }

    fn entities(&self) -> Entities<'q> {
        unreachable!("`Replica` never drives")
    }

    #[inline]
    unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
        self.0.contains(entity).then_some(())
    }
}

/// The live entities a query walks when none of its terms drives: `None`
/// when one does.
struct Alive<'q>(Option<&'q [Entity]>);

// SAFETY: `entities` yields the allocator's live entities, each once;
// `get` hands out no references.
unsafe impl<'q> Joinable<'q> for Alive<'q> {
    type Item = ();

    #[inline]
    fn len(&self) -> usize {
        // below the terms that never drive, above every table
        match self.0 {
            Some(_) => usize::MAX - 1,
            None => usize::MAX,
        }
    }

    fn entities(&self) -> Entities<'q> {
        self.0
            .expect("a query with a driving term lists no live entities")
            .iter()
            .copied()
    }

    #[inline]
    unsafe fn get(&mut self, _: Entity) -> Option<Self::Item> {
        Some(())
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

            const SKIPS_REPLICAS: bool = $($T::SKIPS_REPLICAS)||+;

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

        impl<$($T: Filter),+> Filter for ($($T,)+) {}

        impl<$($T: Filter),+> Term for Or<($($T,)+)> {
            type View<'w> = ($($T::View<'w>,)+);
            type Join<'q, 'w: 'q> = OrJoin<($($T::Join<'q, 'w>,)+)>;
            type Item<'q, 'w: 'q, Rest> = Rest;

            fn fetch(world: &World) -> Self::View<'_> {
                ($($T::fetch(world),)+)
            }

            fn join<'q, 'w>(view: &'q mut Self::View<'w>) -> Self::Join<'q, 'w> {
                let ($($t,)+) = view;
                OrJoin(($($T::join($t),)+))
            }

            #[inline]
            fn item<'q, 'w: 'q, Rest>((): (), rest: Rest) -> Self::Item<'q, 'w, Rest> {
                rest
            }
        }

        impl<$($T: Filter),+> Filter for Or<($($T,)+)> {}

        // SAFETY: `entities` is unreachable (`len` keeps this term from ever
        // being the shortest); each filter is probed at most once per probe of
        // its own, and filters hand out no references.
        unsafe impl<'q, $($T: Joinable<'q>),+> Joinable<'q> for OrJoin<($($T,)+)> {
            type Item = ();

            #[inline]
            fn len(&self) -> usize {
                usize::MAX
            }

            fn entities(&self) -> Entities<'q> {
                unreachable!("`Or` never drives")
            }

            #[inline]
            unsafe fn get(&mut self, entity: Entity) -> Option<Self::Item> {
                let ($($t,)+) = &mut self.0;
                unsafe { ($($t.get(entity).is_some())||+).then_some(()) }
            }
        }
    };
}

/// [`Or`] as a join term: a tuple of filters' join terms, any of which
/// matches.
pub struct OrJoin<J>(J);

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
/// of the same component (another query, a view) panics at
/// [`Query::new`]. [`iter_in`](Self::iter_in) also borrows the cell index,
/// from its first call until the query is dropped.
///
/// A query with a [`Mut`] term over a replicated storage skips replicas, as
/// if filtered by [`Owned`], and borrows the replica flags shared.
pub struct Query<'w, Q: Term> {
    world: &'w World,
    views: Q::View<'w>,
    replicas: Option<Ref<'w, Replicas>>,
    cells: Option<Ref<'w, CellIndex>>,
    /// The live entities, listed by `iter` when no term drives.
    alive: Vec<Entity>,
}

impl<'w, Q: Term> Query<'w, Q> {
    pub fn new(world: &'w World) -> Self {
        Self {
            world,
            views: Q::fetch(world),
            replicas: Q::SKIPS_REPLICAS.then(|| world.entities().replicas()),
            cells: None,
            alive: Vec::new(),
        }
    }

    /// Iterates the entities matching every term, yielding the data terms'
    /// components as a flat tuple - filters are stripped, and a lone data
    /// term yields its component bare.
    ///
    /// A query whose terms all never drive, such as `Option<C>`, `Has<C>`,
    /// `Without<C>` or `Owned`, walks every entity alive when `iter` is
    /// called. The entity allocator is not held, so the loop can spawn, and
    /// the entities it spawns are not visited.
    pub fn iter<'q>(
        &'q mut self,
    ) -> impl Iterator<Item = (Entity, <Q::Item<'q, 'w, Nil> as Flatten>::Flat)>
    where
        Q::Item<'q, 'w, Nil>: Flatten,
    {
        let terms = Q::join(&mut self.views);
        let alive = (terms.len() == usize::MAX).then(|| {
            self.world.entities().alive_into(&mut self.alive);
            self.alive.as_slice()
        });
        let replicas = OwnedIf(self.replicas.as_deref());
        Join::new((terms, Alive(alive), replicas))
            .map(|(entity, (item, (), ()))| (entity, Q::item(item, Nil).flatten()))
    }

    /// `entity`'s item, as [`iter`](Self::iter) would yield it, or `None`
    /// when a term rejects it. When no term drives, a dead entity is
    /// rejected.
    ///
    /// The item borrows the query, so it must be dropped before the next
    /// call.
    pub fn get<'q>(&'q mut self, entity: Entity) -> Option<<Q::Item<'q, 'w, Nil> as Flatten>::Flat>
    where
        Q::Item<'q, 'w, Nil>: Flatten,
    {
        let mut terms = (Q::join(&mut self.views), OwnedIf(self.replicas.as_deref()));
        if terms.0.len() == usize::MAX && !self.world.is_alive(entity) {
            return None;
        }
        // SAFETY: `terms` is built for this call and probed once; the item
        // holds `self` borrowed mutably, so no other probe overlaps it.
        let (item, ()) = unsafe { terms.get(entity) }?;
        Some(Q::item(item, Nil).flatten())
    }

    /// Like [`iter`](Self::iter), over the entities in `cell` only. The
    /// cell's list joins as one more term, so it drives iteration when it is
    /// the shortest.
    ///
    /// Panics if the cell index is borrowed mutably, which only uju does,
    /// between systems.
    pub fn iter_in<'q>(
        &'q mut self,
        cell: Cell,
    ) -> impl Iterator<Item = (Entity, <Q::Item<'q, 'w, Nil> as Flatten>::Flat)>
    where
        Q::Item<'q, 'w, Nil>: Flatten,
    {
        let world = self.world;
        let cells = self
            .cells
            .get_or_insert_with(|| world.entities().cell_index());
        let replicas = OwnedIf(self.replicas.as_deref());
        Join::new((Q::join(&mut self.views), cells.join(cell), replicas))
            .map(|(entity, (item, (), ()))| (entity, Q::item(item, Nil).flatten()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::Component;
    use crate::ecs::entity::{EntityGeneration, EntityIndex};
    use crate::ecs::storage::tracked::Tracked;

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

        // a filter first does not drive
        let mut query = Query::<(Without<Frozen>, Position, With<Velocity>)>::new(&world);
        assert_eq!(
            query.iter().map(|(e, _)| e).collect::<Vec<_>>(),
            [entity(1)]
        );
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
    fn query_owned_replica() {
        crate::ecs::init();
        let world = World::new();
        let entities = (0..4).map(|_| world.spawn()).collect::<Vec<_>>();
        for &entity in &entities {
            world.view_mut::<Position>().insert(entity, Position(0));
        }
        world.entities().set_replica(entities[1], true);
        world.entities().set_replica(entities[2], true);

        // non-schema components stay writable on replicas
        let mut query = Query::<(Mut<Position>, Replica)>::new(&world);
        for (_, position) in query.iter() {
            position.0 = 1;
        }
        drop(query);

        let mut query = Query::<(Position, Owned)>::new(&world);
        let owned = query.iter().map(|(e, p)| (e, p.0)).collect::<Vec<_>>();
        assert_eq!(owned, [(entities[0], 0), (entities[3], 0)]);

        // spawning inside the loop does not conflict with the flags
        let mut query = Query::<(Position, Replica)>::new(&world);
        for (e, position) in query.iter() {
            assert_eq!(position.0, 1);
            assert!(world.is_replica(e));
            world.spawn();
        }
    }

    #[test]
    fn query_iter_in() {
        crate::ecs::init();
        let world = World::new();
        let (a, b) = (Cell::new(1), Cell::new(2));
        let entities = (0..6).map(|_| world.spawn()).collect::<Vec<_>>();
        for (index, &entity) in entities.iter().enumerate() {
            world
                .view_mut::<Position>()
                .insert(entity, Position(index as i32));
            world
                .entities()
                .set_cell(entity, if index < 4 { a } else { b });
        }
        world.view_mut::<Frozen>().insert(entities[2], Frozen);
        world.view_mut::<Frozen>().insert(entities[5], Frozen);
        world.entities().set_replica(entities[1], true);

        let mut query = Query::<Mut<Position>>::new(&world);
        let in_a = query.iter_in(a).map(|(e, _)| e).collect::<Vec<_>>();
        assert_eq!(in_a, entities[..4]);
        for (_, position) in query.iter_in(b) {
            position.0 += 10;
        }
        assert_eq!(query.iter_in(Cell::new(3)).count(), 0);
        drop(query);
        assert_eq!(
            world.view::<Position>().get(entities[5]),
            Some(&Position(15))
        );

        // a smaller table drives, and the cell is a probe
        let mut query = Query::<(Position, With<Frozen>)>::new(&world);
        let frozen = query.iter_in(b).map(|(e, p)| (e, p.0)).collect::<Vec<_>>();
        assert_eq!(frozen, [(entities[5], 15)]);
        drop(query);

        let mut query = Query::<Owned>::new(&world);
        let owned = query.iter_in(a).map(|(e, ())| e).collect::<Vec<_>>();
        assert_eq!(owned, [entities[0], entities[2], entities[3]]);
        let mut query = Query::<(Position, Replica)>::new(&world);
        let replicas = query.iter_in(a).map(|(e, _)| e).collect::<Vec<_>>();
        assert_eq!(replicas, [entities[1]]);
    }

    #[test]
    fn query_iter_in_spawns() {
        crate::ecs::init();
        let world = World::new();
        let cell = Cell::new(0);
        let e = world.spawn();
        world.view_mut::<Position>().insert(e, Position(0));
        world.entities().set_cell(e, cell);

        let mut query = Query::<Position>::new(&world);
        for (e, _) in query.iter_in(cell) {
            let spawned = world.spawn();
            world.commands().insert(spawned, Position(1));
            assert_eq!(world.cell(e), Some(cell));
        }
        drop(query);
        world.flush();
        assert_eq!(world.view::<Position>().len(), 2);
    }

    #[test]
    #[should_panic(expected = "already borrowed")]
    fn set_cell_inside_iter_in_panics() {
        crate::ecs::init();
        let world = World::new();
        let cell = Cell::new(0);
        let e = world.spawn();
        world.view_mut::<Position>().insert(e, Position(0));
        world.entities().set_cell(e, cell);

        let mut query = Query::<Position>::new(&world);
        for (e, _) in query.iter_in(cell) {
            world.entities().set_cell(e, Cell::new(1));
        }
    }

    #[test]
    fn query_without_driver_walks_alive() {
        crate::ecs::init();
        let world = World::new();
        let entities = (0..4).map(|_| world.spawn()).collect::<Vec<_>>();
        world.despawn(entities[0]);
        world.view_mut::<Frozen>().insert(entities[2], Frozen);
        world.entities().set_replica(entities[3], true);

        let mut query = Query::<Without<Frozen>>::new(&world);
        let thawed = query.iter().map(|(e, ())| e).collect::<Vec<_>>();
        assert_eq!(thawed, [entities[1], entities[3]]);
        drop(query);

        // spawning inside the loop is free, and the spawned are not visited
        let mut query = Query::<Owned>::new(&world);
        let mut owned = Vec::new();
        for (e, ()) in query.iter() {
            owned.push(e);
            world.spawn();
        }
        assert_eq!(owned, [entities[1], entities[2]]);
        assert_eq!(query.iter().count(), 4);
    }

    #[test]
    fn query_option_has() {
        crate::ecs::init();
        let world = World::new();
        let entities = (0..4).map(|_| world.spawn()).collect::<Vec<_>>();
        for &entity in &entities[..3] {
            world.view_mut::<Position>().insert(entity, Position(1));
        }
        world
            .view_mut::<Velocity>()
            .insert(entities[1], Velocity(5));
        world
            .view_mut::<Velocity>()
            .insert(entities[3], Velocity(6));
        world.view_mut::<Frozen>().insert(entities[2], Frozen);

        // `Position` drives; the optional terms never filter
        let mut query = Query::<(Mut<Position>, Option<Velocity>, Has<Frozen>)>::new(&world);
        for (_, (position, velocity, frozen)) in query.iter() {
            if !frozen {
                position.0 += velocity.map_or(0, |v| v.0);
            }
        }
        drop(query);
        let positions = world.view::<Position>();
        let positions = entities[..3]
            .iter()
            .map(|&e| positions.get(e).unwrap().0)
            .collect::<Vec<_>>();
        assert_eq!(positions, [1, 6, 1]);

        // optional terms alone walk every live entity
        let mut query = Query::<(Option<Mut<Velocity>>, Has<Position>)>::new(&world);
        for (_, (velocity, has_position)) in query.iter() {
            if let Some(velocity) = velocity
                && !has_position
            {
                velocity.0 = 0;
            }
        }
        let matched = query
            .iter()
            .map(|(e, (velocity, has))| (e, velocity.map(|v| v.0), has))
            .collect::<Vec<_>>();
        assert_eq!(
            matched,
            [
                (entities[0], None, true),
                (entities[1], Some(5), true),
                (entities[2], None, true),
                (entities[3], Some(0), false),
            ]
        );
    }

    #[test]
    fn query_or() {
        crate::ecs::init();
        let world = World::new();
        let entities = (0..5).map(|_| world.spawn()).collect::<Vec<_>>();
        for &entity in &entities {
            world.view_mut::<Position>().insert(entity, Position(0));
        }
        world
            .view_mut::<Velocity>()
            .insert(entities[1], Velocity(0));
        world.view_mut::<Frozen>().insert(entities[2], Frozen);
        world
            .view_mut::<Velocity>()
            .insert(entities[3], Velocity(0));
        world.view_mut::<Frozen>().insert(entities[3], Frozen);
        world.entities().set_replica(entities[4], true);

        let mut query = Query::<(Position, Or<(With<Velocity>, With<Frozen>)>)>::new(&world);
        let matched = query.iter().map(|(e, _)| e).collect::<Vec<_>>();
        assert_eq!(matched, entities[1..4]);
        drop(query);

        // a nested tuple matches when all of its filters do
        let mut query = Query::<Or<((With<Velocity>, With<Frozen>), Replica)>>::new(&world);
        let matched = query.iter().map(|(e, ())| e).collect::<Vec<_>>();
        assert_eq!(matched, [entities[3], entities[4]]);
    }

    #[test]
    fn query_get() {
        crate::ecs::init();
        let world = World::new();
        let entities = (0..3).map(|_| world.spawn()).collect::<Vec<_>>();
        world
            .view_mut::<Position>()
            .insert(entities[0], Position(1));
        world
            .view_mut::<Position>()
            .insert(entities[1], Position(2));
        world.view_mut::<Frozen>().insert(entities[1], Frozen);

        let mut query = Query::<(Mut<Position>, Without<Frozen>)>::new(&world);
        query.get(entities[0]).unwrap().0 += 10;
        query.get(entities[0]).unwrap().0 += 10;
        assert!(query.get(entities[1]).is_none());
        assert!(query.get(entities[2]).is_none());
        drop(query);
        assert_eq!(
            world.view::<Position>().get(entities[0]),
            Some(&Position(21))
        );

        // with no driving term, a dead entity is rejected
        let mut query = Query::<Has<Position>>::new(&world);
        assert_eq!(query.get(entities[2]), Some(false));
        drop(query);
        world.despawn(entities[2]);
        assert_eq!(Query::<Has<Position>>::new(&world).get(entities[2]), None);
    }

    #[derive(Component, Debug, PartialEq)]
    #[component(storage = Tracked<Self>)]
    struct Hp(u32);

    #[test]
    fn query_added() {
        crate::ecs::init();
        let mut world = World::new();
        let entities = (0..4).map(|_| world.spawn()).collect::<Vec<_>>();
        for &entity in &entities[..2] {
            world.view_mut::<Hp>().insert(entity, Hp(0));
        }
        world.end_tick();

        world.view_mut::<Hp>().get_mut(entities[0]).unwrap().0 = 1;
        world.view_mut::<Hp>().insert(entities[2], Hp(2));

        let mut query = Query::<(Hp, Added<Hp>)>::new(&world);
        let added = query.iter().map(|(e, _)| e).collect::<Vec<_>>();
        assert_eq!(added, [entities[2]]);
        drop(query);

        // an added value is changed too
        let mut query = Query::<(Hp, Changed<Hp>)>::new(&world);
        let mut changed = query.iter().map(|(e, _)| e).collect::<Vec<_>>();
        changed.sort();
        assert_eq!(changed, [entities[0], entities[2]]);
        drop(query);

        let mut query = Query::<Or<(Added<Hp>, Without<Hp>)>>::new(&world);
        let matched = query.iter().map(|(e, ())| e).collect::<Vec<_>>();
        assert_eq!(matched, [entities[2], entities[3]]);
    }

    #[test]
    fn world_removed() {
        crate::ecs::init();
        let mut world = World::new();
        let entities = (0..5).map(|_| world.spawn()).collect::<Vec<_>>();
        for &entity in &entities {
            world.view_mut::<Hp>().insert(entity, Hp(0));
        }
        world.end_tick();

        world.view_mut::<Hp>().remove(entities[3]);
        world.view_mut::<Hp>().remove(entities[0]);
        // removed, then held again
        world.view_mut::<Hp>().remove(entities[1]);
        world.view_mut::<Hp>().insert(entities[1], Hp(1));
        // removed, then despawned
        world.view_mut::<Hp>().remove(entities[2]);
        world.despawn(entities[2]);
        // a despawn alone is no removal
        world.despawn(entities[4]);

        let removed = world.removed::<Hp>().collect::<Vec<_>>();
        assert_eq!(removed, [entities[3], entities[0]]);

        // spawning inside the loop is free
        for _ in world.removed::<Hp>() {
            let spawned = world.spawn();
            world.commands().insert(spawned, Hp(0));
        }
        world.flush();

        world.end_tick();
        assert_eq!(world.removed::<Hp>().count(), 0);
    }
}
