# Schedule

How a `World`'s systems run: stages, their order, run conditions, sets, and
when [commands](commands.md) are applied. A `Schedule` runs on the
`World`'s thread, one system at a time.

```rust
use uju::prelude::*;

let mut schedule = Schedule::new();
schedule
    .system(Update, movement)
    .system(Update, combat.run_if(in_battle))
    .systems(Update, (regen, decay).run_if(every(10)))
    .system(Inbox, on_hit);

schedule.run(&world);
```

## Systems

A system is a function of the `World`:

```rust
fn movement(world: &World) {
    let mut query = Query::<(Mut<Position>, Velocity)>::new(world);
    for (_, (position, velocity)) in query.iter() {
        position.0 += velocity.0;
    }
}
```

- **Shape.** Any `FnMut(&World) + 'static`: a `fn` item, or a closure that
  may keep state of its own between runs.
- **Closures name their argument's type**, as in `|world: &World| { ... }`,
  since the schedule takes any system type and cannot infer it.
- **Panics are not caught.** A panicking system unwinds out of the run.

## Stages

| Stage | Holds |
|---|---|
| `Update` | Gameplay |
| `Inbox` | The systems that read inboxes, after `Update` |

`Stage` is an enum, and `Update` and `Inbox` are re-exported beside it and
in the prelude. A schedule that needs a single stage, such as an edge's,
uses `Update` alone.

| Method | Runs |
|---|---|
| `schedule.run_stage(stage, &world)` | The systems of one stage |
| `schedule.run(&world)` | Every stage, in the order of `Stage::ALL`: `Update`, then `Inbox` |

`run_stage` lets the code that owns the schedule do its own work between
the stages, such as applying what arrived before `Update`, or
[setting cells](entities.md#setting-cells) after `Inbox`.

## Order

- **Order of registration.** Inside a stage, systems run in the order they
  were added, whether by `system` or `systems`. Nothing is solved from
  declared dependencies, so reading the registrations gives the order.
- **Each run, every system once.** A stage runs each of its systems at
  most once per `run_stage`, so a system that reads what a later one
  writes sees it on the next run.

## Run conditions

A run condition is any `FnMut(&World) -> bool + 'static`. `run_if` attaches
one to a system or a set:

```rust
fn in_battle(world: &World) -> bool {
    world.unique::<Battle>().active
}

schedule.system(Update, combat.run_if(in_battle));
schedule.system(Update, flee.run_if(|world| !in_battle(world)));
```

- **Checked before each run.** A condition that does not hold skips the
  system for this run.
- **Several conditions** must all hold. They are checked in the order they
  were attached, and the first that fails skips the rest.
- **`every(n)`** holds once in `n` ticks: when the `Time` unique's
  `ticks()` is a multiple of `n`, so on the first tick and every `n`th
  after. A `World` without `Time` makes it count its own checks instead.
  `every(0)` panics.

## Sets

A tuple of up to 12 systems is a set, added with `systems`:

```rust
schedule.systems(Update, (regen, decay, cleanup).run_if(every(10)));
```

- **One place in the order.** A set's systems run where the set was
  added, in tuple order.
- **Conditions are checked once** for the whole set, before its first
  system. Each system may carry conditions of its own as well.
- **Sets nest.** An item of a tuple may be a set, with its own conditions:

```rust
schedule.systems(
    Update,
    (movement, (combat, loot).run_if(in_battle)).run_if(not_paused),
);
```

`system` takes a single system, with or without conditions, and `systems`
takes a set or a single system.

## Flush

The schedule applies the [command queue](commands.md#flush) for its systems:

- **After every system** that runs, so the next system sees what it
  queued. A system's queries and views are dropped by then. An empty queue
  costs one check.
- **When a stage starts**, so commands queued outside any system, such as
  by the runtime before `Update`, show to the stage's first system.

So commands are applied on both sides of `Inbox`: its systems see what
`Update` spawned and despawned, and what they defer is applied before
`run_stage` returns.
