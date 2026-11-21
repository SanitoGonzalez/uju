use crate::ecs::world::World;
use crate::time::Time;

use node::{Body, Node};

/// A stage of a [`Schedule`]. [`run`](Schedule::run) runs the stages in
/// the order they are declared here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    /// Gameplay.
    Update,
    /// The systems that read inboxes, after `Update`.
    Inbox,
}

pub use Stage::{Inbox, Update};

impl Stage {
    /// Every stage, in the order they run.
    pub const ALL: [Stage; 2] = [Update, Inbox];
}

/// Systems in stages, each stage in order of registration.
///
/// A stage applies the [command queue](World::commands) when it starts and
/// after every system it runs.
pub struct Schedule {
    stages: [Vec<Node>; Stage::ALL.len()],
}

impl Schedule {
    pub fn new() -> Self {
        Self {
            stages: Default::default(),
        }
    }

    /// Adds one system, with its run conditions, at the end of `stage`.
    pub fn system(&mut self, stage: Stage, system: impl IntoSystem) -> &mut Self {
        self.stages[stage as usize].push(system.into_node());
        self
    }

    /// Adds a set of systems, with its run conditions, at the end of
    /// `stage`.
    pub fn systems(&mut self, stage: Stage, systems: impl IntoSystems) -> &mut Self {
        self.stages[stage as usize].push(systems.into_node());
        self
    }

    /// Applies the command queue, then runs the systems of `stage` in order,
    /// each whose conditions hold, applying the queue after each.
    ///
    /// A panic in a system or a flush is not caught.
    pub fn run_stage(&mut self, stage: Stage, world: &World) {
        world.flush();
        for node in &mut self.stages[stage as usize] {
            node.run(world);
        }
    }

    /// Runs every stage, in the order of [`Stage::ALL`].
    pub fn run(&mut self, world: &World) {
        for stage in Stage::ALL {
            self.run_stage(stage, world);
        }
    }
}

impl Default for Schedule {
    fn default() -> Self {
        Self::new()
    }
}

/// A system, or a set of systems, that a [`Schedule`] can hold: a
/// `FnMut(&World)`, a tuple of up to 12 of them, or either with run
/// conditions. A tuple's items may be tuples, so sets nest.
pub trait IntoSystems: Sized + 'static {
    /// What [`run_if`](Self::run_if) returns: a [`System`] for a system, a
    /// [`Set`] for a set.
    type Conditioned: IntoSystems;

    /// Runs `self` only when `condition` holds, checked once before each
    /// run, after the conditions already added. For a set, it is checked
    /// once for all its systems.
    fn run_if(self, condition: impl FnMut(&World) -> bool + 'static) -> Self::Conditioned;

    #[doc(hidden)]
    fn into_node(self) -> Node;
}

/// A single system: a `FnMut(&World)`, or one with run conditions. What
/// [`Schedule::system`] takes.
pub trait IntoSystem: IntoSystems {}

/// A system with its run conditions.
pub struct System(Node);

/// A set of systems with its run conditions.
pub struct Set(Node);

impl<F: FnMut(&World) + 'static> IntoSystems for F {
    type Conditioned = System;

    fn run_if(self, condition: impl FnMut(&World) -> bool + 'static) -> System {
        System(self.into_node()).run_if(condition)
    }

    fn into_node(self) -> Node {
        Node::new(Body::System(Box::new(self)))
    }
}

impl<F: FnMut(&World) + 'static> IntoSystem for F {}

impl IntoSystems for System {
    type Conditioned = System;

    fn run_if(mut self, condition: impl FnMut(&World) -> bool + 'static) -> System {
        self.0.conditions.push(Box::new(condition));
        self
    }

    fn into_node(self) -> Node {
        self.0
    }
}

impl IntoSystem for System {}

impl IntoSystems for Set {
    type Conditioned = Set;

    fn run_if(mut self, condition: impl FnMut(&World) -> bool + 'static) -> Set {
        self.0.conditions.push(Box::new(condition));
        self
    }

    fn into_node(self) -> Node {
        self.0
    }
}

macro_rules! impl_set {
    ($($T:ident $t:ident),+) => {
        impl<$($T: IntoSystems),+> IntoSystems for ($($T,)+) {
            type Conditioned = Set;

            fn run_if(self, condition: impl FnMut(&World) -> bool + 'static) -> Set {
                Set(self.into_node()).run_if(condition)
            }

            fn into_node(self) -> Node {
                let ($($t,)+) = self;
                Node::new(Body::Set(vec![$($t.into_node()),+]))
            }
        }
    };
}

impl_set!(T0 t0);
impl_set!(T0 t0, T1 t1);
impl_set!(T0 t0, T1 t1, T2 t2);
impl_set!(T0 t0, T1 t1, T2 t2, T3 t3);
impl_set!(T0 t0, T1 t1, T2 t2, T3 t3, T4 t4);
impl_set!(T0 t0, T1 t1, T2 t2, T3 t3, T4 t4, T5 t5);
impl_set!(T0 t0, T1 t1, T2 t2, T3 t3, T4 t4, T5 t5, T6 t6);
impl_set!(T0 t0, T1 t1, T2 t2, T3 t3, T4 t4, T5 t5, T6 t6, T7 t7);
impl_set!(T0 t0, T1 t1, T2 t2, T3 t3, T4 t4, T5 t5, T6 t6, T7 t7, T8 t8);
impl_set!(T0 t0, T1 t1, T2 t2, T3 t3, T4 t4, T5 t5, T6 t6, T7 t7, T8 t8, T9 t9);
impl_set!(T0 t0, T1 t1, T2 t2, T3 t3, T4 t4, T5 t5, T6 t6, T7 t7, T8 t8, T9 t9, T10 t10);
impl_set!(
    T0 t0, T1 t1, T2 t2, T3 t3, T4 t4, T5 t5, T6 t6, T7 t7, T8 t8, T9 t9, T10 t10, T11 t11
);

/// A run condition that holds once in `n` ticks: on the ticks whose
/// [`Time::ticks`] is a multiple of `n`. With no `Time` in the world, it
/// counts its own checks instead, holding on the first and once in `n`
/// after.
///
/// Panics if `n` is `0`.
pub fn every(n: u64) -> impl FnMut(&World) -> bool + 'static {
    assert_ne!(n, 0, "every(0) never holds");
    let mut checks = 0;
    move |world| match world.get_unique::<Time>() {
        Some(time) => time.ticks() % n == 0,
        None => {
            let holds = checks % n == 0;
            checks += 1;
            holds
        }
    }
}

mod node {
    use crate::ecs::world::World;

    type Condition = Box<dyn FnMut(&World) -> bool>;

    /// A system or a set, with the conditions that gate it.
    pub struct Node {
        pub(super) conditions: Vec<Condition>,
        pub(super) body: Body,
    }

    pub(super) enum Body {
        System(Box<dyn FnMut(&World)>),
        Set(Vec<Node>),
    }

    impl Node {
        pub(super) fn new(body: Body) -> Self {
            Self {
                conditions: Vec::new(),
                body,
            }
        }

        pub(super) fn run(&mut self, world: &World) {
            if !self.conditions.iter_mut().all(|condition| condition(world)) {
                return;
            }
            match &mut self.body {
                Body::System(system) => {
                    system(world);
                    world.flush();
                }
                Body::Set(nodes) => {
                    for node in nodes {
                        node.run(world);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    use super::*;
    use crate::ecs::query::Query;

    #[derive(crate::ecs::Component, Debug, PartialEq)]
    struct Hp(u32);

    #[derive(crate::ecs::Component, Debug, PartialEq)]
    struct Hit;

    #[derive(crate::ecs::Unique, Default)]
    struct Log(Vec<&'static str>);

    fn log(world: &World, entry: &'static str) {
        world.unique_mut::<Log>().0.push(entry);
    }

    fn logged(world: &World) -> Vec<&'static str> {
        std::mem::take(&mut world.unique_mut::<Log>().0)
    }

    fn world() -> World {
        crate::ecs::init();
        let world = World::new();
        world.insert_unique(Log::default());
        world
    }

    #[test]
    fn schedule_order() {
        let world = world();
        let mut schedule = Schedule::new();
        schedule
            .system(Inbox, |world: &World| log(world, "inbox"))
            .system(Update, |world: &World| log(world, "a"))
            .systems(
                Update,
                (
                    |world: &World| log(world, "b"),
                    |world: &World| log(world, "c"),
                ),
            )
            .system(Update, |world: &World| log(world, "d"));

        schedule.run(&world);
        assert_eq!(logged(&world), ["a", "b", "c", "d", "inbox"]);

        schedule.run_stage(Inbox, &world);
        assert_eq!(logged(&world), ["inbox"]);
    }

    fn movement(world: &World) {
        log(world, "movement");
    }

    fn combat(world: &World) {
        log(world, "combat");
    }

    fn in_battle(_: &World) -> bool {
        false
    }

    fn regen(world: &World) {
        log(world, "regen");
    }

    fn decay(world: &World) {
        log(world, "decay");
    }

    fn on_hit(world: &World) {
        log(world, "on_hit");
    }

    #[test]
    fn schedule_fn_items() {
        let world = world();
        world.insert_unique(Time::new());
        let mut schedule = Schedule::new();
        schedule
            .system(Update, movement)
            .system(Update, combat.run_if(in_battle))
            .systems(Update, (regen, decay).run_if(every(10)))
            .system(Inbox, on_hit);

        schedule.run(&world);
        assert_eq!(logged(&world), ["movement", "regen", "decay", "on_hit"]);
        world
            .unique_mut::<Time>()
            .advance(Duration::from_millis(50));
        schedule.run(&world);
        assert_eq!(logged(&world), ["movement", "on_hit"]);
    }

    #[test]
    fn schedule_flushes_after_every_system() {
        let world = world();
        let e = world.spawn();
        world.view_mut::<Hp>().insert(e, Hp(0));

        let mut schedule = Schedule::new();
        schedule.systems(
            Update,
            (
                // a query's borrow is dropped by the time the flush runs
                |world: &World| {
                    let mut query = Query::<Hp>::new(world);
                    for (e, hp) in query.iter() {
                        if hp.0 == 0 {
                            world.commands().insert(e, Hit);
                        }
                    }
                },
                move |world: &World| {
                    assert!(world.view::<Hit>().contains(e));
                    world.commands().despawn(e);
                },
            ),
        );
        schedule.system(Update, move |world: &World| assert!(!world.is_alive(e)));

        schedule.run(&world);
        assert!(world.commands().is_empty());
    }

    #[test]
    fn schedule_flushes_around_inbox() {
        let world = world();
        let mut schedule = Schedule::new();
        schedule.system(Inbox, |world: &World| {
            let mut query = Query::<Hp>::new(world);
            for (e, _) in query.iter() {
                log(world, "seen");
                world.commands().insert(e, Hit);
            }
        });

        // queued outside any system, as by the runtime before the stages
        let e = world.spawn();
        world.commands().insert(e, Hp(1));
        schedule.run_stage(Update, &world);
        assert!(world.view::<Hp>().contains(e));

        world.commands().remove::<Hp>(e);
        let f = world.spawn();
        world.commands().insert(f, Hp(2));
        schedule.run_stage(Inbox, &world);
        assert_eq!(logged(&world), ["seen"]);
        assert!(!world.view::<Hit>().contains(e));
        assert!(world.view::<Hit>().contains(f));
        assert!(world.commands().is_empty());
    }

    #[test]
    fn schedule_run_if() {
        let world = world();
        let open = Rc::new(RefCell::new(false));
        let checks = Rc::new(RefCell::new(0));

        let mut schedule = Schedule::new();
        {
            let open = open.clone();
            let checks = checks.clone();
            schedule
                .system(
                    Update,
                    (|world: &World| log(world, "always")).run_if(|_| true),
                )
                .system(
                    Update,
                    (|world: &World| log(world, "gated")).run_if(move |_| *open.borrow()),
                )
                .systems(
                    Update,
                    (
                        |world: &World| log(world, "a"),
                        |world: &World| log(world, "b"),
                        |world: &World| log(world, "c"),
                    )
                        .run_if(move |_| {
                            *checks.borrow_mut() += 1;
                            true
                        }),
                );
        }

        schedule.run(&world);
        assert_eq!(logged(&world), ["always", "a", "b", "c"]);
        assert_eq!(*checks.borrow(), 1);

        *open.borrow_mut() = true;
        schedule.run(&world);
        assert_eq!(logged(&world), ["always", "gated", "a", "b", "c"]);
        assert_eq!(*checks.borrow(), 2);
    }

    #[test]
    fn schedule_run_if_chained() {
        let world = world();
        let checked = Rc::new(RefCell::new(Vec::new()));
        let (first, second) = (checked.clone(), checked.clone());

        let mut schedule = Schedule::new();
        schedule.system(
            Update,
            (|world: &World| log(world, "ran"))
                .run_if(move |_| {
                    first.borrow_mut().push("first");
                    false
                })
                .run_if(move |_| {
                    second.borrow_mut().push("second");
                    true
                }),
        );

        schedule.run(&world);
        assert!(logged(&world).is_empty());
        assert_eq!(*checked.borrow(), ["first"]);
    }

    #[test]
    fn schedule_nested_sets() {
        let world = world();
        let mut schedule = Schedule::new();
        schedule.systems(
            Update,
            (
                |world: &World| log(world, "a"),
                (
                    |world: &World| log(world, "b"),
                    |world: &World| log(world, "c"),
                )
                    .run_if(|_| false),
                (|world: &World| log(world, "d"),),
            )
                .run_if(|_| true),
        );

        schedule.run(&world);
        assert_eq!(logged(&world), ["a", "d"]);
    }

    #[test]
    fn schedule_stateful_system() {
        let world = world();
        let mut runs = 0;
        let mut schedule = Schedule::new();
        schedule.system(Update, move |world: &World| {
            runs += 1;
            if runs == 2 {
                log(world, "second");
            }
        });

        schedule.run(&world);
        schedule.run(&world);
        schedule.run(&world);
        assert_eq!(logged(&world), ["second"]);
    }

    #[test]
    fn every_reads_time() {
        let world = world();
        world.insert_unique(Time::new());
        let mut schedule = Schedule::new();
        schedule.systems(
            Update,
            (
                |world: &World| log(world, "a"),
                |world: &World| log(world, "b"),
            )
                .run_if(every(3)),
        );

        let mut ran = Vec::new();
        for _ in 0..7 {
            ran.push(!logged(&world).is_empty());
            schedule.run(&world);
            world
                .unique_mut::<Time>()
                .advance(Duration::from_millis(50));
        }
        ran.push(!logged(&world).is_empty());
        assert_eq!(ran, [false, true, false, false, true, false, false, true]);

        // the tick counter decides, not how often it was checked
        let mut every = every(3);
        assert!(!every(&world));
        world.unique_mut::<Time>().advance(Duration::ZERO);
        assert!(!every(&world));
        world.unique_mut::<Time>().advance(Duration::ZERO);
        assert!(every(&world));
    }

    #[test]
    fn every_without_time() {
        let world = world();
        let mut every = every(2);
        let held = (0..5).map(|_| every(&world)).collect::<Vec<_>>();
        assert_eq!(held, [true, false, true, false, true]);
    }

    #[test]
    #[should_panic(expected = "every(0)")]
    fn every_zero() {
        let _ = every(0);
    }
}
