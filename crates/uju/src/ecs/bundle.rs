use crate::ecs::component::{Component, Required};
use crate::ecs::entity::Entity;
use crate::ecs::storage::Storage;
use crate::ecs::world::World;

/// Components inserted together: a component, or a tuple of up to 12
/// bundles. A tuple's items may be tuples, so bundles nest.
///
/// A component that appears twice is inserted twice, in order, so the last
/// value wins.
pub trait Bundle: 'static {
    /// The values of a run of inserts, a column per component, as
    /// [`Commands`](crate::ecs::commands::Commands) keeps them.
    #[doc(hidden)]
    type Columns: Default + 'static;

    /// Inserts each component into its table, borrowing each table once.
    #[doc(hidden)]
    fn insert(self, world: &World, entity: Entity);

    #[doc(hidden)]
    fn push(self, columns: &mut Self::Columns);

    /// Inserts the columns, a row per entity of `entities`, skipping the
    /// rows whose entity is `None`, and borrowing each table once.
    #[doc(hidden)]
    fn insert_columns(columns: Self::Columns, world: &World, entities: &[Option<Entity>]);

    /// Adds the components each component of the bundle requires.
    #[doc(hidden)]
    fn required(required: &mut Required<'_>);
}

impl<C: Component> Bundle for C {
    type Columns = Vec<C>;

    #[inline]
    fn insert(self, world: &World, entity: Entity) {
        Storage::insert(&mut *world.view_mut::<C>(), entity, self);
    }

    #[inline]
    fn push(self, columns: &mut Vec<C>) {
        columns.push(self);
    }

    fn insert_columns(column: Vec<C>, world: &World, entities: &[Option<Entity>]) {
        let mut table = world.view_mut::<C>();
        for (&entity, value) in entities.iter().zip(column) {
            if let Some(entity) = entity {
                Storage::insert(&mut *table, entity, value);
            }
        }
    }

    #[inline]
    fn required(required: &mut Required<'_>) {
        C::required(required);
    }
}

macro_rules! impl_bundle {
    ($($B:ident $i:tt),+) => {
        impl<$($B: Bundle),+> Bundle for ($($B,)+) {
            type Columns = ($($B::Columns,)+);

            #[inline]
            fn insert(self, world: &World, entity: Entity) {
                $(self.$i.insert(world, entity);)+
            }

            #[inline]
            fn push(self, columns: &mut Self::Columns) {
                $(self.$i.push(&mut columns.$i);)+
            }

            fn insert_columns(columns: Self::Columns, world: &World, entities: &[Option<Entity>]) {
                $($B::insert_columns(columns.$i, world, entities);)+
            }

            #[inline]
            fn required(required: &mut Required<'_>) {
                $($B::required(required);)+
            }
        }
    };
}

impl_bundle!(B0 0);
impl_bundle!(B0 0, B1 1);
impl_bundle!(B0 0, B1 1, B2 2);
impl_bundle!(B0 0, B1 1, B2 2, B3 3);
impl_bundle!(B0 0, B1 1, B2 2, B3 3, B4 4);
impl_bundle!(B0 0, B1 1, B2 2, B3 3, B4 4, B5 5);
impl_bundle!(B0 0, B1 1, B2 2, B3 3, B4 4, B5 5, B6 6);
impl_bundle!(B0 0, B1 1, B2 2, B3 3, B4 4, B5 5, B6 6, B7 7);
impl_bundle!(B0 0, B1 1, B2 2, B3 3, B4 4, B5 5, B6 6, B7 7, B8 8);
impl_bundle!(B0 0, B1 1, B2 2, B3 3, B4 4, B5 5, B6 6, B7 7, B8 8, B9 9);
impl_bundle!(B0 0, B1 1, B2 2, B3 3, B4 4, B5 5, B6 6, B7 7, B8 8, B9 9, B10 10);
impl_bundle!(B0 0, B1 1, B2 2, B3 3, B4 4, B5 5, B6 6, B7 7, B8 8, B9 9, B10 10, B11 11);

#[cfg(test)]
mod tests {
    use std::cell::UnsafeCell;

    use linkme::distributed_slice;

    use crate::ecs::Component;
    use crate::ecs::component::{COMPONENTS, Component, Id, Registration, Required, new_table};
    use crate::ecs::query::Query;
    use crate::ecs::storage::sparse_set::SparseSet;
    use crate::ecs::world::World;

    #[derive(Component, Debug, Clone, PartialEq)]
    struct Hp(u32);

    #[derive(Component, Debug, Clone, PartialEq)]
    struct Loot(u32);

    #[derive(Component, Debug, PartialEq)]
    struct Hostile;

    #[derive(Component, Debug, PartialEq)]
    #[component(require(Body = Body(1), Mass))]
    struct Collider;

    #[derive(Component, Debug, PartialEq)]
    #[component(require(Mass = Mass(2)))]
    struct Body(u32);

    #[derive(Component, Debug, Default, PartialEq)]
    struct Mass(u32);

    // a cycle: each requires the other
    #[derive(Component, Debug, PartialEq)]
    #[component(require(Right = Right(1)))]
    struct Left(u32);

    #[derive(Component, Debug, PartialEq)]
    #[component(require(Left = Left(2)))]
    struct Right(u32);

    /// A hand-written component with a requirement.
    #[derive(Debug, PartialEq)]
    struct Armor(u32);

    #[distributed_slice(COMPONENTS)]
    static ARMOR: Registration = Registration {
        name: concat!(module_path!(), "::Armor"),
        id: UnsafeCell::new(u16::MAX),
        new_table: new_table::<Armor>,
    };

    impl Component for Armor {
        type Storage = SparseSet<Armor>;

        fn id() -> Id {
            unsafe { *ARMOR.id.get() }
        }

        fn required(required: &mut Required<'_>) {
            required.add(|| Hp(50));
        }
    }

    #[test]
    fn spawn_with_bundles() {
        crate::ecs::init();
        let world = World::new();

        let one = world.spawn_with(Hp(1));
        let e = world.spawn_with((Hp(2), Loot(3), Hostile));
        // nested, and the last of a repeated component wins
        let nested = world.spawn_with(((Hp(4), Loot(5)), Hp(6)));

        let hps = world.view::<Hp>();
        let loots = world.view::<Loot>();
        assert_eq!(hps.get(one), Some(&Hp(1)));
        assert_eq!(loots.get(one), None);
        assert_eq!(hps.get(e), Some(&Hp(2)));
        assert_eq!(loots.get(e), Some(&Loot(3)));
        assert!(world.view::<Hostile>().contains(e));
        assert_eq!(hps.get(nested), Some(&Hp(6)));
        assert_eq!(loots.get(nested), Some(&Loot(5)));
    }

    #[test]
    fn commands_insert_bundles() {
        crate::ecs::init();
        let world = World::new();
        let e0 = world.spawn();
        let e1 = world.spawn();
        let dead = world.spawn();
        world.despawn(dead);

        {
            let mut commands = world.commands();
            commands.insert(e0, (Hp(1), Loot(1)));
            commands.insert(dead, (Hp(2), Loot(2)));
            commands.insert(e1, (Hp(3), Loot(3)));
            commands.insert(e0, (Hp(4), Loot(4)));
            commands.insert(e1, Hp(5));
            commands.insert(e1, ((Loot(6), Hostile), Loot(7)));
        }
        world.flush();

        let hps = world.view::<Hp>();
        let loots = world.view::<Loot>();
        assert_eq!(hps.get(e0), Some(&Hp(4)));
        assert_eq!(loots.get(e0), Some(&Loot(4)));
        assert_eq!(hps.get(e1), Some(&Hp(5)));
        assert_eq!(loots.get(e1), Some(&Loot(7)));
        assert!(world.view::<Hostile>().contains(e1));
        assert_eq!(hps.len(), 2);
        assert_eq!(loots.len(), 2);
    }

    #[test]
    fn required_components() {
        crate::ecs::init();
        let world = World::new();

        // transitive: `Collider` requires `Body`, which requires `Mass`, and
        // `Collider`'s own `Mass` wins, being nearer
        let e = world.spawn_with(Collider);
        assert_eq!(world.view::<Body>().get(e), Some(&Body(1)));
        assert_eq!(world.view::<Mass>().get(e), Some(&Mass(0)));

        let e = world.spawn_with(Body(7));
        assert_eq!(world.view::<Mass>().get(e), Some(&Mass(2)));
        assert!(!world.view::<Collider>().contains(e));

        // what the bundle holds is not overwritten, wherever it sits
        let e = world.spawn_with((Collider, Body(9)));
        assert_eq!(world.view::<Body>().get(e), Some(&Body(9)));
        assert_eq!(world.view::<Mass>().get(e), Some(&Mass(0)));

        // nor what the entity held before
        let e = world.spawn_with(Mass(5));
        world.commands().insert(e, Collider);
        world.flush();
        assert_eq!(world.view::<Body>().get(e), Some(&Body(1)));
        assert_eq!(world.view::<Mass>().get(e), Some(&Mass(5)));

        // a cycle stops at the component held
        let e = world.spawn_with(Left(0));
        assert_eq!(world.view::<Right>().get(e), Some(&Right(1)));
        assert_eq!(world.view::<Left>().get(e), Some(&Left(0)));

        // hand-written
        let e = world.spawn_with(Armor(3));
        assert_eq!(world.view::<Hp>().get(e), Some(&Hp(50)));

        // a view's insert holds one table, and adds nothing
        let e = world.spawn();
        world.view_mut::<Collider>().insert(e, Collider);
        assert!(!world.view::<Body>().contains(e));
    }

    #[test]
    fn required_skips_replicas() {
        crate::ecs::init();
        let world = World::new();
        let replica = world.spawn();
        world.entities().set_replica(replica, true);
        let owned = world.spawn();

        world.commands().insert(replica, Collider);
        world.commands().insert(owned, Collider);
        world.flush();
        assert!(world.view::<Collider>().contains(replica));
        assert!(!world.view::<Body>().contains(replica));
        assert!(!world.view::<Mass>().contains(replica));
        assert_eq!(world.view::<Body>().get(owned), Some(&Body(1)));
    }

    #[test]
    fn spawn_inside_query() {
        crate::ecs::init();
        let world = World::new();
        world.spawn_with((Hp(0), Loot(4)));
        world.spawn_with(Hp(1));

        let mut drops = Vec::new();
        let mut query = Query::<(Hp, Loot)>::new(&world);
        for (_, (hp, loot)) in query.iter() {
            if hp.0 == 0 {
                let drop = world.spawn();
                world.commands().insert(drop, (loot.clone(), Collider));
                drops.push(drop);
            }
        }
        drop(query);
        world.flush();

        assert_eq!(world.view::<Loot>().get(drops[0]), Some(&Loot(4)));
        assert_eq!(world.view::<Body>().get(drops[0]), Some(&Body(1)));
    }

    #[test]
    #[should_panic(expected = "already borrowed")]
    fn spawn_with_inside_query_panics() {
        crate::ecs::init();
        let world = World::new();
        world.spawn_with(Hp(0));

        let mut query = Query::<Hp>::new(&world);
        for _ in query.iter() {
            world.spawn_with((Loot(1), Hp(1)));
        }
    }
}
