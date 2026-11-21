# Schedule

How a `World`'s systems run: stages, their order, run conditions, sets, and
when [commands](commands.md) are applied and
[component hooks](components.md#hooks) run, and the [timers](#timers)
systems count down. A `Schedule` runs on the `World`'s thread, one system
at a time.

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
| `Inbox` | The systems that read [inboxes](events.md#inboxes), after `Update` |

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
  writes sees it on the next run. An [event](events.md#events) emitted
  after its last reader ran is never read.

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

The schedule [flushes](commands.md#flush) for its systems: it applies the
command queue and runs the [component hooks](components.md#hooks).

- **After every system** that runs, so the next system sees what it
  queued, and the hooks of what it changed have run. A system's queries
  and views are dropped by then. A flush with nothing to do costs a few
  checks.
- **When a stage starts**, so commands queued outside any system, such as
  by the runtime before `Update`, show to the stage's first system, and the
  hooks of what was changed outside it, such as replicas the runtime
  received, have run.

So commands are applied on both sides of `Inbox`: its systems see what
`Update` spawned and despawned, and what they defer is applied before
`run_stage` returns.

## Timers

A `Timer`, in `uju::time` and the prelude, holds a count of ticks left. A
system counts it down once per tick:

```rust
#[derive(Component)]
struct Cooldown(Timer);

fn cooldowns(world: &World) {
    let mut query = Query::<Mut<Cooldown>>::new(world);
    for (_, cooldown) in query.iter() {
        if cooldown.0.tick() {
            // ready again
        }
    }
}
```

- **What is left, not an end tick.** Shards' tick counters are unrelated,
  so a deadline in ticks would mean nothing once its entity migrates. A
  timer moves with its entity and keeps counting.
- **Once.** `tick()` subtracts one and returns `true` on the call that
  reaches zero. A finished timer stays at zero and returns `false`, so
  `true` comes once per start. `Timer::new(0)` and `Timer::default()` are
  finished, and never return `true`.
- **Restarting** is assigning a new one, as `cooldown.0 = Timer::new(30)`.
- **Ticks, not time.** Each `tick()` is one tick, whatever its duration.
  The tick count and the last tick's duration are in the `Time` unique, as
  `ticks()` and `delta()`.

| Item | Gives |
|---|---|
| `Timer::new(ticks)` | A timer finishing after `ticks` calls to `tick()`. `ticks` is a `u32` |
| `timer.tick()` | Counts down one tick, `true` when it reaches zero |
| `timer.remaining()` | The ticks left |
| `timer.is_finished()` | Whether no tick is left |
| `From<u8>`, `From<u16>`, `From<u32>` | A timer of that many ticks |
| `u32::from(timer)`, `u64::from(timer)` | The ticks left |

### Timers in the schema

A schema stores a timer as a plain unsigned integer, through a `type` the
Rust side maps to `Timer` as a
[foreign type](../protocol/schema.md#foreign-types):

```
namespace demo

type Ticks = u32
```

```toml
[rust.map]
"demo.Ticks" = "::uju::time::Timer"
```

```rust
// `Ticks` is the generated type of `demo.Ticks`.
impl From<Ticks> for Timer {
    fn from(ticks: Ticks) -> Self {
        Timer::new(ticks.0)
    }
}

impl From<Timer> for Ticks {
    fn from(timer: Timer) -> Self {
        Ticks(timer.remaining())
    }
}
```

- **Derives.** `Timer` derives what a `type` over an integer does:
  `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`, `PartialOrd`, `Ord`, `Hash`
  and `Default`.
- **Narrower integers.** For a `u8` or `u16`, the conversion back narrows
  `remaining()`. A timer that came from the field only counts down, so it
  always fits.
- **Every tick is a change.** A timer in a `replace` or `patch` component
  is written each tick, so it marks its component dirty each tick and
  sends it. One that clients only need to start from is better sent once,
  as a duration, and counted down on each side.
