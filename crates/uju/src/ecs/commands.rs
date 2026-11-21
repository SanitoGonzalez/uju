use std::any::{Any, TypeId};
use std::ops::Range;

use crate::ecs::bundle::Bundle;
use crate::ecs::component::{Component, Id, Required};
use crate::ecs::entity::Entity;
use crate::ecs::world::World;

/// Changes deferred until the [`World`] applies them, for code that holds
/// borrows a direct change would conflict with, such as a query's.
///
/// [`World::commands`] hands out the queue, and [`World::flush`] applies
/// it. Commands apply in the order they were queued.
#[derive(Default)]
pub struct Commands {
    queue: Vec<Command>,
    /// The entities of queued despawns and removes, each run of them a
    /// range, so a run is applied as one slice.
    entities: Vec<Entity>,
}

enum Command {
    Despawn(Range<usize>),
    Remove(Id, Range<usize>),
    /// A run of inserts of one bundle type.
    Insert(TypeId, Box<dyn InsertRun>),
    Run(Box<dyn FnOnce(&World)>),
}

/// A run of inserts of bundle `B`, its values kept a column per component.
struct Inserts<B: Bundle> {
    entities: Vec<Option<Entity>>,
    columns: B::Columns,
}

trait InsertRun {
    fn as_any_mut(&mut self) -> &mut dyn Any;

    fn apply(self: Box<Self>, world: &World);
}

impl<B: Bundle> InsertRun for Inserts<B> {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn apply(self: Box<Self>, world: &World) {
        let Inserts {
            mut entities,
            columns,
        } = *self;
        {
            let registry = world.entities.entries();
            for entity in &mut entities {
                *entity = entity.filter(|&entity| registry.is_alive(entity));
            }
        }
        B::insert_columns(columns, world, &entities);
        for &entity in entities.iter().flatten() {
            Required::add_to::<B>(world, entity);
        }
    }
}

impl Commands {
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Despawns `entity`, if it is still alive when applied.
    pub fn despawn(&mut self, entity: Entity) {
        let end = self.entities.len();
        self.entities.push(entity);
        match self.queue.last_mut() {
            Some(Command::Despawn(run)) if run.end == end => run.end += 1,
            _ => self.queue.push(Command::Despawn(end..end + 1)),
        }
    }

    /// Inserts `bundle` into `entity`, replacing the components it already
    /// holds, then adds the components they
    /// [require](Component::required). Skipped if `entity` is dead when
    /// applied.
    pub fn insert<B: Bundle>(&mut self, entity: Entity, bundle: B) {
        let id = TypeId::of::<B>();
        if let Some(Command::Insert(run_id, run)) = self.queue.last_mut()
            && *run_id == id
        {
            let run = run.as_any_mut().downcast_mut::<Inserts<B>>().unwrap();
            run.entities.push(Some(entity));
            bundle.push(&mut run.columns);
            return;
        }
        let mut run = Inserts::<B> {
            entities: vec![Some(entity)],
            columns: B::Columns::default(),
        };
        bundle.push(&mut run.columns);
        self.queue.push(Command::Insert(id, Box::new(run)));
    }

    /// Removes `entity`'s `C`, if it holds one when applied.
    pub fn remove<C: Component>(&mut self, entity: Entity) {
        let id = C::id();
        let end = self.entities.len();
        self.entities.push(entity);
        match self.queue.last_mut() {
            Some(Command::Remove(run_id, run)) if *run_id == id && run.end == end => run.end += 1,
            _ => self.queue.push(Command::Remove(id, end..end + 1)),
        }
    }

    /// Runs `f` on the world. Commands it queues apply in the same flush,
    /// after those already queued.
    pub fn run(&mut self, f: impl FnOnce(&World) + 'static) {
        self.queue.push(Command::Run(Box::new(f)));
    }

    /// Applies and clears the queue. Runs of despawns, of removes of one
    /// component and of inserts of one bundle type each borrow what they
    /// touch once, but for the components the inserts require.
    pub(crate) fn apply(&mut self, world: &World) {
        for command in self.queue.drain(..) {
            match command {
                Command::Despawn(run) => {
                    world.despawn_batch(&self.entities[run]);
                }
                Command::Remove(id, run) => {
                    world.tables[id as usize]
                        .borrow_mut()
                        .remove_batch(&self.entities[run]);
                }
                Command::Insert(_, run) => run.apply(world),
                Command::Run(f) => f(world),
            }
        }
        self.entities.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::ecs::query::{Query, With};

    #[derive(crate::ecs::Component, Debug, Clone, PartialEq)]
    struct Hp(u32);

    #[derive(crate::ecs::Component, Debug, Clone, PartialEq)]
    struct Loot(u32);

    #[derive(crate::ecs::Component, Debug, PartialEq)]
    struct Marked;

    #[test]
    fn commands_inside_query() {
        crate::ecs::init();
        let world = World::new();
        let alive = world.spawn();
        let dying = world.spawn();
        world.view_mut::<Hp>().insert(alive, Hp(5));
        world.view_mut::<Hp>().insert(dying, Hp(0));
        world.view_mut::<Loot>().insert(dying, Loot(9));

        let mut drops = Vec::new();
        let mut query = Query::<(Hp, Loot)>::new(&world);
        for (e, (hp, loot)) in query.iter() {
            if hp.0 == 0 {
                let drop = world.spawn();
                world.commands().insert(drop, loot.clone());
                world.commands().despawn(e);
                drops.push(drop);
            }
        }
        drop(query);

        // nothing shows before the flush
        assert!(world.is_alive(dying));
        assert_eq!(world.view::<Hp>().get(dying), Some(&Hp(0)));
        assert_eq!(world.view::<Loot>().get(drops[0]), None);

        world.flush();
        assert!(!world.is_alive(dying));
        assert!(!world.view::<Hp>().contains(dying));
        assert!(!world.view::<Loot>().contains(dying));
        assert_eq!(world.view::<Loot>().get(drops[0]), Some(&Loot(9)));
        assert_eq!(world.view::<Hp>().get(alive), Some(&Hp(5)));
        assert!(world.commands().is_empty());
    }

    #[test]
    fn commands_apply_in_order() {
        crate::ecs::init();
        let world = World::new();
        let e0 = world.spawn();
        let e1 = world.spawn();
        let e2 = world.spawn();

        {
            let mut commands = world.commands();
            commands.insert(e0, Hp(1));
            commands.remove::<Hp>(e0);
            commands.remove::<Hp>(e1);
            commands.insert(e1, Hp(2));
            // a despawned entity gets nothing inserted after
            commands.despawn(e2);
            commands.insert(e2, Hp(3));
            commands.insert(e0, Hp(4));
            commands.insert(e0, Hp(5));
        }
        world.flush();

        let hps = world.view::<Hp>();
        assert_eq!(hps.get(e0), Some(&Hp(5)));
        assert_eq!(hps.get(e1), Some(&Hp(2)));
        assert!(!world.is_alive(e2));
        assert_eq!(hps.entities(), &[e1, e0]);
    }

    #[test]
    fn commands_merge_runs() {
        crate::ecs::init();
        let world = World::new();
        let entities = (0..6).map(|_| world.spawn()).collect::<Vec<_>>();
        for &entity in &entities {
            world.view_mut::<Hp>().insert(entity, Hp(1));
            world.view_mut::<Loot>().insert(entity, Loot(1));
        }

        {
            let mut commands = world.commands();
            commands.despawn(entities[0]);
            commands.despawn(entities[1]);
            commands.remove::<Hp>(entities[2]);
            commands.remove::<Hp>(entities[3]);
            commands.remove::<Loot>(entities[3]);
            commands.despawn(entities[4]);
            commands.despawn(entities[4]);

            let runs = commands
                .queue
                .iter()
                .map(|command| match command {
                    Command::Despawn(run) => ("despawn", run.clone()),
                    Command::Remove(_, run) => ("remove", run.clone()),
                    _ => unreachable!(),
                })
                .collect::<Vec<_>>();
            assert_eq!(
                runs,
                [
                    ("despawn", 0..2),
                    ("remove", 2..4),
                    ("remove", 4..5),
                    ("despawn", 5..7)
                ]
            );
        }
        world.flush();

        let hps = world.view::<Hp>();
        let loots = world.view::<Loot>();
        for index in [0, 1, 4] {
            assert!(!world.is_alive(entities[index]));
        }
        assert_eq!(hps.entities(), &[entities[5]]);
        let mut held = loots.entities().to_vec();
        held.sort();
        assert_eq!(held, [entities[2], entities[5]]);
    }

    #[test]
    fn commands_merge_insert_runs() {
        crate::ecs::init();
        let world = World::new();
        let entities = (0..4).map(|_| world.spawn()).collect::<Vec<_>>();

        let mut commands = world.commands();
        commands.insert(entities[0], (Hp(1), Loot(1)));
        commands.insert(entities[1], (Hp(1), Loot(1)));
        commands.insert(entities[2], Hp(1));
        commands.insert(entities[3], Hp(1));
        commands.insert(entities[0], (Loot(1), Hp(1)));
        commands.insert(entities[1], (Loot(1), Hp(1)));
        let runs = commands
            .queue
            .iter()
            .map(|command| match command {
                Command::Insert(id, _) => *id,
                _ => unreachable!(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            runs,
            [
                TypeId::of::<(Hp, Loot)>(),
                TypeId::of::<Hp>(),
                TypeId::of::<(Loot, Hp)>()
            ]
        );
    }

    #[test]
    fn commands_queued_while_flushing() {
        crate::ecs::init();
        let world = World::new();
        let e = world.spawn();
        let log = Rc::new(RefCell::new(Vec::new()));

        {
            let log = log.clone();
            world.commands().run(move |world| {
                log.borrow_mut().push("outer");
                let inner = log.clone();
                world.commands().run(move |world| {
                    inner.borrow_mut().push("inner");
                    assert_eq!(world.view::<Hp>().get(e), Some(&Hp(1)));
                });
                world.commands().insert(e, Marked);
            });
        }
        {
            let log = log.clone();
            world.commands().insert(e, Hp(1));
            world
                .commands()
                .run(move |_| log.borrow_mut().push("after"));
        }
        world.flush();

        assert_eq!(*log.borrow(), ["outer", "after", "inner"]);
        assert!(world.view::<Marked>().contains(e));
        assert!(world.commands().is_empty());

        // empty queue: nothing to do
        world.flush();
        let mut query = Query::<With<Marked>>::new(&world);
        assert_eq!(query.iter().count(), 1);
    }

    #[test]
    fn despawn_batch() {
        crate::ecs::init();
        let world = World::new();
        let entities = (0..4).map(|_| world.spawn()).collect::<Vec<_>>();
        for &entity in &entities {
            world.view_mut::<Hp>().insert(entity, Hp(1));
        }
        world.despawn(entities[3]);

        let batch = [entities[0], entities[3], entities[1], entities[0]];
        assert_eq!(world.despawn_batch(&batch), 2);
        assert_eq!(world.despawn_batch(&batch), 0);
        assert_eq!(world.view::<Hp>().entities(), &[entities[2]]);

        // a recycled slot is not hit by its stale handle
        let recycled = world.spawn();
        world.view_mut::<Hp>().insert(recycled, Hp(2));
        assert_eq!(world.despawn_batch(&batch), 0);
        assert_eq!(world.view::<Hp>().get(recycled), Some(&Hp(2)));
    }

    #[test]
    #[should_panic(expected = "already borrowed")]
    fn flush_inside_query_panics() {
        crate::ecs::init();
        let world = World::new();
        let e = world.spawn();
        world.view_mut::<Hp>().insert(e, Hp(0));

        let mut query = Query::<Hp>::new(&world);
        for (e, _) in query.iter() {
            world.commands().despawn(e);
            world.flush();
        }
    }
}
