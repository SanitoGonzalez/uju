# Events and inboxes

How systems pass values to later systems in the same `World`. An event is
read by any number of systems in the tick it was emitted, then cleared. An
entity message is sent to one entity and read once, by its owner, from the
message type's inbox. Both are queues on the `World`, one per type, each a
borrow of its own. Where the systems that read them run is in
[Schedule](schedule.md).

```rust
world.emit(Died { entity, killer });
for died in world.events::<Died>().iter() { ... }

world.send(target, &Hit { amount, source });
for (e, hit) in world.inbox::<Hit>() { ... }
```

## Events

An event is a plain Rust type, with no schema, deriving `Event`:

```rust
#[derive(Event)]
struct Died {
    entity: Entity,
    killer: Entity,
}

fn combat(world: &World) {
    // ...
    world.emit(Died { entity, killer });
}

fn score(world: &World) {
    for died in world.events::<Died>().iter() {
        // ...
    }
}
```

| Method | Does |
|---|---|
| `world.emit(event)` | Pushes `event` onto its type's queue, at once |
| `world.events::<E>()` | Borrows `E`'s queue as a slice, in the order emitted |

- **Registered like components.** `#[derive(Event)]` gives the type a
  queue on every `World`. Generic types cannot derive it.
- **Many readers.** `events` is a shared borrow, so any number of systems,
  or loops, read the queue at once.
- **Emitting while reading panics.** `emit::<E>` borrows `E`'s queue
  mutably, so emitting `E` inside a loop over `events::<E>()` panics, like
  any conflicting borrow. A system that must do so collects first. Queues
  of other types are separate borrows, so a loop over one type can emit
  another.
- **Cleared at the end of the tick.** An event emitted after its last
  reader ran is never read, so the order of
  [registration](schedule.md#order) matters.
- **Local.** Events never leave the `World`. What must reach another
  shard's entity is an [entity message](#inboxes).

## Inboxes

Gameplay that acts on an entity it may not own, such as a hit, sends the
entity a message. The entity's owner reads it from the message type's
inbox and changes the state, so the sender never branches on
[`is_replica`](entities.md#replicas).

```rust
fn fireball(world: &World) {
    // ...
    world.send(target, &Hit { amount: 40, source });
}

fn on_hit(world: &World) {
    for (e, hit) in world.inbox::<Hit>() {
        // e is owned and alive
    }
}
```

An entity message is a type implementing `EntityMessage` and `Clone`. Each
has an inbox on every `World`.

| Method | Does |
|---|---|
| `world.send(target, &message)` | Sends a copy of `message` to `target` |
| `world.inbox::<M>()` | Drains `M`'s inbox, yielding `(Entity, M)` |

### Sending

- **To an owned entity**, `send` pushes onto the inbox at once, so a
  system later in the tick reads it.
- **To a replica**, `send` queues the message with the replica's uid for
  uju's runtime, which takes it to the shard that owns the entity. It does
  not reach this `World`'s inbox.
- **To a dead entity**, the message is dropped.
- **From other shards**, uju's runtime pushes the messages they sent onto
  the inbox when the tick begins.

### Reading

- **Drained, not cleared.** `inbox` takes the inbox's messages and yields
  each with its entity, oldest first. Nothing is cleared unread: the end of
  the tick keeps the inbox as it is.
- **Dead entities are skipped**, checked as each message is yielded, so a
  message to an entity despawned by an earlier message's handling is
  dropped.
- **No loop.** The inbox is free while it is read, so sending `M` inside a
  loop over `inbox::<M>()` does not panic, and the loop does not yield it.
  It stays queued, read by a later system of the tick that reads `M`, or
  on the next tick.
- **Dropped early**, the drain puts the messages it did not yield back in
  front of the inbox, ahead of any sent meanwhile.

The `Inbox` [stage](schedule.md#stages) runs the systems that read inboxes,
after `Update`, so a message sent in `Update` to an owned entity is read in
the tick that sent it. A message an `Inbox` system sends, such as a
reflected hit, is read in the same tick only if a system reading its type
runs later in the stage:

```rust
schedule
    .system(Inbox, on_hit)   // sends Hit back: read on the next tick
    .system(Inbox, on_heal); // reads the Heal on_hit sent, this tick
```
