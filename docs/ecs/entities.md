# Entities

How a `World` names its entities. An entity has a local handle, its
[`Entity`](#entity), and may have a cluster-wide name, its [`Uid`](#uid).
The [`Registry`](#registry) keeps both and the map between them.

## Entity

An `Entity` is a `World`'s local handle: a `u32` index and a non-zero `u32`
generation, 8 bytes, aligned to 8.

- **Local.** An `Entity` means nothing on another `World`. To name an
  entity outside its `World`, use its [`Uid`](#uid).
- **Recycled.** A despawned entity's index is reused by a later spawn with a
  new generation, so a stale handle never reaches the new entity. A slot
  whose generation runs out is retired.
- **Niche.** The generation is never zero, so `Option<Entity>` stays 8
  bytes.
- **Bits.** `to_bits` is `generation << 32 | index`. `from_bits` panics on a
  zero generation.

```rust
let world = World::new();
let e = world.spawn();
assert!(world.is_alive(e));
assert!(world.despawn(e));
assert!(!world.is_alive(e));
```

## Spawning and despawning

| Method | Does |
|---|---|
| `world.spawn()` | Allocates an entity with no components, at once |
| `world.despawn(entity)` | Frees the entity and drops its components. `false` if it was dead |
| `world.despawn_batch(&entities)` | Despawns every live entity of the slice, and returns how many |
| `world.commands().despawn(entity)` | Despawns at the next [flush](commands.md) |

- **Spawning is always immediate.** The registry is a borrow of its own, so
  a query's loop can spawn and hold the new entity at once.
- **Despawning touches every table.** `despawn` and `despawn_batch` panic
  when a view or a query holds any table. Inside a query's loop,
  [`commands().despawn`](commands.md) defers it.
- **Batches.** `despawn_batch` borrows each table once for the whole
  slice, and skips tables that hold nothing. Dead entities and repeats in
  the slice are skipped.

## Uid

A `Uid` names an entity across the cluster, and stays the same when the
entity migrates. It is a `u64`:

```
uid := incarnation:u32 << 32 | serial:u32
```

| Part | Meaning |
|---|---|
| `incarnation` | The shard instance that minted the uid. Never zero. |
| `serial` | Counted from `0` per incarnation. |

- **Niche.** The incarnation is never zero, so `Option<Uid>` stays 8 bytes.
- **What it tells.** The incarnation names the shard instance that minted
  the entity, not the shard that owns it now.
- **One spawn.** A uid lasts one spawn of an entity. A lasting identity,
  such as a character's database key, is the user's own type.
- **Bits.** `Uid::new(incarnation, serial)`, `to_bits` and `from_bits`
  convert. `from_bits` panics on a zero incarnation.
- **Display.** A uid prints as `incarnation:serial`, such as `3:42`.

```rust
let uid = Uid::new(NonZeroU32::new(3).unwrap(), 42);
assert_eq!(uid.to_bits(), 3 << 32 | 42);
assert_eq!(uid.incarnation().get(), 3);
assert_eq!(uid.serial(), 42);
```

## Minting and adopting

An entity gets a uid in one of two ways.

- **Minting.** `world.promote(entity)` gives the entity's uid, and mints one
  from the `World`'s [`UidAllocator`](#uidallocator) on first use. A spawn
  mints nothing, so entities never named outside the `World` cost no uid.
  `promote` returns `None` when the entity is dead, or when it has no uid
  and no allocator is installed.
- **Adopting.** `world.adopt(uid)` spawns a local entity for a uid minted
  elsewhere, such as an entity migrating in or a replica. It returns `None`
  when the `World` already holds `uid`.

An adopted entity keeps its uid: `promote` on it returns the adopted uid and
mints nothing.

| Method | Returns | `None` when |
|---|---|---|
| `world.uid(entity)` | The entity's uid | The entity is dead, or has no uid |
| `world.promote(entity)` | The entity's uid, minted on first use | The entity is dead, or has no uid and no allocator is installed |
| `world.adopt(uid)` | A new local entity carrying `uid` | The `World` already holds `uid` |
| `world.resolve(uid)` | The local entity carrying `uid` | The `World` does not hold `uid` |

## UidAllocator

A `UidAllocator` mints uids from two incarnations: the **current** one,
counting serials from `0`, and a **spare**. Each incarnation holds
`UidAllocator::SERIALS`, 2³², serials.

- **Optional.** `World::new()` installs none, so a `World` that only
  receives entities, such as an edge's, can only adopt.
  `world.install_uid_allocator(UidAllocator::new(current, spare))` installs
  one and returns the previous one.
- **Switching.** When the current incarnation has no serial left, the next
  mint switches to the spare, counts from `0` again, and records the
  incarnation it switched from. `remaining()` counts the serials left in
  the current incarnation.
- **Refilling.** The allocator never asks for incarnations itself. Its
  owner takes the incarnations it switched from with `take_retired()`, and
  hands it the next spare with `set_spare(incarnation)`, which returns the
  spare it replaces, if any. `world.uid_allocator_mut()` borrows the
  installed allocator for both.
- **Exhaustion.** `promote` panics when the current incarnation has run out
  and no spare is set. `try_alloc` on the allocator itself returns `None`
  instead.
- **Nothing to persist.** Serials live in memory only. Each incarnation
  must be handed to one allocator only, so a restarted shard mints from new
  incarnations rather than resuming old ones.

| Method | Gives |
|---|---|
| `incarnation()` | The current incarnation |
| `spare()` | The spare, or `None` once switched to and not yet replaced |
| `set_spare(incarnation)` | Sets the spare, returning the previous one |
| `remaining()` | Serials left in the current incarnation |
| `take_retired()` | The incarnations switched from since the last call, oldest first |
| `alloc()`, `try_alloc()` | The next uid. `alloc` panics where `try_alloc` returns `None` |

```rust
let incarnation = |n| NonZeroU32::new(n).unwrap();
let world = World::new();
world.install_uid_allocator(UidAllocator::new(incarnation(3), incarnation(4)));
let e = world.spawn();
let uid = world.promote(e).unwrap();
assert_eq!(world.resolve(uid), Some(e));

// each retired incarnation means the spare is in use: get a new one
let retired = world.uid_allocator_mut().map(|mut uids| uids.take_retired());

// later, with the new incarnation
world.uid_allocator_mut().unwrap().set_spare(incarnation(5));
```

## Registry

The `Registry` owns a `World`'s entities. `world.entities()` borrows it, and
the `World`'s methods above forward to it.

- **Entity to uid** is a slot per entity index, cleared on despawn, so a
  recycled index never inherits a uid.
- **Uid to entity** is a hash map. Every item a receiver applies goes
  through it, so it uses a fast integer hasher in place of SipHash. The
  hasher does not resist flooding: uids come from uju's own servers.
- **Despawn** frees the entity and forgets its uid. `resolve` on that uid
  returns `None` from then on.
