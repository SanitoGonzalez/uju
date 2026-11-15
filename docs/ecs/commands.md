# Commands

How to change entities while borrows forbid it. A query holds its tables
until it is dropped, so a change to one of them inside its loop, or a
despawn, which touches every table, panics. `Commands` queue such changes,
and the `World` applies them later. Tables and views are in
[Components](components.md), queries in [Queries](queries.md).

```rust
let mut q = Query::<(Hp, Loot)>::new(&world);
for (e, (hp, loot)) in q.iter() {
    if hp.0 == 0 {
        let bag = world.spawn();                  // immediate
        world.commands().insert(bag, loot.clone());
        world.commands().despawn(e);
    }
}
drop(q);
world.flush();
```

## The queue

`world.commands()` borrows the `World`'s queue. The queue is a borrow of its
own, so it is available inside any query's loop or while views are held.

| Method | Applies as |
|---|---|
| `despawn(entity)` | `world.despawn(entity)`. Skipped if the entity is dead |
| `insert(entity, value)` | Inserts or replaces the entity's `C`. Skipped if the entity is dead |
| `remove::<C>(entity)` | Removes the entity's `C`, if it holds one |
| `run(f)` | Calls `f(&World)` |
| `is_empty()` | Whether anything is queued |

- **Hold it briefly.** Each call to `world.commands()` borrows the queue,
  and a borrow held across a `flush` panics there. Chain one call per
  command, or drop the borrow first.
- **Closures** are `FnOnce(&World) + 'static`, for any change the queue
  does not name, such as a unique or an entity's [uid](entities.md#uid).
- **Spawning stays immediate.** `world.spawn()` only borrows the registry,
  so a loop spawns at once, holds the new entity, and queues its
  components.

## Flush

`world.flush()` applies the queue in the order commands were queued, then
leaves it empty.

- **Deferred means unseen.** A queued change shows only once applied. A
  queued despawn leaves the entity alive and readable until then, and a
  queued insert leaves the table as it was.
- **Order.** Commands apply in the order they were queued. A despawn
  followed by an insert to the same entity inserts nothing, and an insert
  followed by a despawn drops the inserted value.
- **Commands queued while flushing**, such as by a closure, apply in the
  same flush, after every command queued before them. A flush returns once
  the queue is empty.
- **Borrows.** A flush borrows the tables its commands touch, so it panics
  inside a query's loop that holds one of them, or while a view of one is
  held. An empty queue costs one check.
- **No implicit flush.** Nothing applies the queue on its own: code that
  queues commands calls `flush` once its borrows are dropped.

## Batches

A flush applies runs of similar commands together, so each run borrows what
it touches once rather than once per command:

| Run | Borrows |
|---|---|
| Consecutive despawns | The registry once, then each table once, as one [`despawn_batch`](entities.md#spawning-and-despawning) |
| Consecutive removes of one component | That component's table once |
| Consecutive inserts of one component | That component's table and the registry once |

A despawn of a run costs a probe per table, not a `RefCell` borrow per table
per entity. A command of another kind ends the run, so batching never
reorders commands.

## Immediate changes

The `World`'s own methods stay, for code that holds no conflicting borrow:

| Immediate | Deferred |
|---|---|
| `world.despawn(entity)`, `world.despawn_batch(&entities)` | `world.commands().despawn(entity)` |
| `world.view_mut::<C>().insert(entity, value)` | `world.commands().insert(entity, value)` |
| `world.view_mut::<C>().remove(entity)` | `world.commands().remove::<C>(entity)` |
