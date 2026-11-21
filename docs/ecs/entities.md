# Entities

How a `World` names its entities. An entity has a local handle, its
[`Entity`](#entity), and may have a cluster-wide name, its [`Uid`](#uid),
and a [`Cell`](#cells). It is owned by its `World` or is a
[replica](#replicas) of one another shard owns. The [`Registry`](#registry)
keeps all of it.

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
| `world.spawn_with(bundle)` | Spawns an entity holding a [bundle](components.md#bundles), at once. Panics when a query or a view holds one of its tables |
| `world.despawn(entity)` | Frees the entity and drops its components. `false` if it was dead |
| `world.despawn_batch(&entities)` | Despawns every live entity of the slice, and returns how many |
| `world.commands().despawn(entity)` | Despawns at the next [flush](commands.md) |

- **Spawning is always immediate.** The entity allocator is a borrow of
  its own, so a query's loop can spawn and hold the new entity at once.
  `spawn_with` also borrows the bundle's tables, so inside a query's loop,
  `spawn` then [`commands().insert`](commands.md) gives the entity its
  components at the next flush.
- **Despawning touches everything.** `despawn` and `despawn_batch` drop the
  entity from every table, [audience tables](audiences.md) included, its
  cell and the replica flags, so they panic
  while any view or query is alive. Inside a query's loop,
  [`commands().despawn`](commands.md) defers it.
- **Despawns are logged.** The `World` notes each despawn of an owned
  entity that was in a cell at the start of the tick, with its uid and that
  cell, so replication can tell receivers it is gone. Replicas and entities
  never in a cell are not noted.
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
  mint switches to the spare and counts from `0` again. `remaining()`
  counts the serials left in the current incarnation.
- **The switch is an event.** When `world.promote` switches the installed
  allocator, the `World` emits an [event](events.md#events),
  `IncarnationSwitched { from }`, naming the incarnation it switched from,
  whether the promote ran in a system or outside the schedule. Like every
  event, it is cleared at the end of the tick, so its reader runs before.
- **Refilling.** The allocator never asks for incarnations itself. Its
  owner hands it the next spare with `set_spare(incarnation)`, which
  returns the spare it replaces, if any. `world.uid_allocator_mut()`
  borrows the installed allocator.
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
| `alloc()`, `try_alloc()` | The next uid. `alloc` panics where `try_alloc` returns `None` |

```rust
let incarnation = |n| NonZeroU32::new(n).unwrap();
let world = World::new();
world.install_uid_allocator(UidAllocator::new(incarnation(3), incarnation(4)));
let e = world.spawn();
let uid = world.promote(e).unwrap();
assert_eq!(world.resolve(uid), Some(e));

// each switch means the spare is in use: get a new one
for switched in world.events::<IncarnationSwitched>().iter() {
    // switched.from ran out: ask for a new spare
}

// later, with the new incarnation
world.uid_allocator_mut().unwrap().set_spare(incarnation(5));
```

## Replicas

A sim's `World` holds the entities it owns and, near its borders, replicas
of entities other shards own. An edge's `World` holds replicas only.

- **A flag, not a component.** The registry keeps a bit per entity index.
  Only uju's runtime sets or clears it, between systems. A despawn clears
  it, so a recycled index starts owned.
- **Reading it.** `world.is_replica(entity)` reads the flag, `false` for a
  dead entity. The [`Owned` and `Replica`](queries.md#terms) query filters
  keep or skip replicas.
- **Reads see both.** Queries without a filter yield owned entities and
  replicas alike, so gameplay can target and collide across a border.
- **Writes skip them.** Mutable access to a replicated component, through a
  `Mut<C>` term or a view's `get_mut`, skips replicas, since the owner's
  next delta would overwrite the write. Components outside the schema stay
  writable on replicas. See [Change tracking](change-tracking.md#replicas).

```rust
let mut q = Query::<(Mut<Hp>, Owned)>::new(&world);
for (e, hp) in q.iter() {
    hp.0 += 1; // never a replica
}
```

## Cells

An entity's cell is a `Cell`, a plain type holding a `u32` cell id. It is
not a component: the registry keeps it beside the entity's uid.

- **uju sets it**, from the entity's `Transform`, as [Setting
  cells](#setting-cells) says. Gameplay reads it with `world.cell(entity)`
  and never sets it.
- **None** for a dead entity, and for one uju has not placed in a cell.
- **Despawn** forgets the cell, so a recycled index starts without one.
- **Start of tick.** The registry also keeps the cell each entity had when
  the tick began, so replication can tell an entity that moved from one
  that entered.

| Method | Gives |
|---|---|
| `Cell::new(id)`, `cell.id()` | Converts from and to the `u32` id |
| `world.cell(entity)` | The entity's cell, or `None` |
| `world.entities().cell_index()` | The [cell index](#cell-index), borrowed |

### Grid

A `Grid`, a unique of the `World`, cuts the world into square cells of side
`size`. It cuts the ground plane: `x` and `y` in 2D, `x` and `z` in 3D,
where `y` is up. Cell `(0, 0)` starts at the origin.

```
id := (row + 32768):u16 << 16 | (column + 32768):u16

column := floor(x / size)        row := floor(y / size), or z in 3D
```

- **Coordinates** run from `-32768` to `32767` on each axis. A position
  past the last cell of an axis falls in it.
- **Margin.** An entity leaves its cell only once it is more than `margin`
  past one of the cell's borders, on either axis. One standing on a line
  keeps its cell rather than switching every tick.
- **Checked.** `Grid::new(size, margin)` panics unless `size` is finite
  and positive, and `margin` finite and not negative.

| Method | Gives |
|---|---|
| `Grid::new(size, margin)` | A grid |
| `grid.size()`, `grid.margin()` | The cell side and the margin |
| `grid.cell(position)` | The cell holding `position` |
| `grid.cell_from(cell, position)` | `cell` while `position` is within the margin around it, else `grid.cell(position)` |

```rust
let grid = Grid::new(64.0, 4.0);
let cell = grid.cell(Vector::new(10.0, 70.0)); // column 0, row 1 in 2D
assert_eq!(cell.id(), 32769 << 16 | 32768);
// 2 past the border: within the margin
assert_eq!(grid.cell_from(cell, Vector::new(10.0, 62.0)), cell);
```

### Setting cells

uju sets cells once per tick, before replication encodes it. It walks the
entities whose `Transform` was added or written this tick, so its cost
follows what moved. `world.cell(entity)` reads the cell as of that point.

- **First seen.** An entity with no cell, or whose `Transform` was added
  this tick, takes the cell holding its position, and gets its uid, as by
  `promote`. This is when it enters replication.
- **Moving.** Any other entity keeps its cell while within the margin, and
  takes the cell holding its position once past it.
- **Replicas** are skipped. The runtime sets their cell as they enter.
- **No `Transform`, no cell.** An entity that never had a `Transform` is
  never placed in a cell and is never replicated. One that loses its
  `Transform` keeps the cell it had.
- **Only writes count.** Reading through a mutable `Transform` marks
  nothing, so it moves nothing. See [Change
  tracking](change-tracking.md#tracked-tables).

### Cell index

The registry keeps, per cell, a dense list of the entities in it, and per
entity its position in that list. A cell change or a despawn updates it
with a `swap_remove`, in O(1).

- **Replicas are listed too.**
- **Queries by cell.** [`query.iter_in(cell)`](queries.md#iter_in) joins a
  cell's list as one more term.
- **Reading it.** `cell_index().entities(cell)` is the cell's list, in no
  promised order, empty for a cell with no entity. `cells()` iterates the
  cells holding at least one entity, and `contains(cell, entity)` probes.
- **Stable within a system.** Only uju reorders a cell's list, between
  systems, so no loop sees it change.

## Registry

The `Registry` owns a `World`'s entities. `world.entities()` returns it, and
the `World`'s methods above forward to it, except `promote`, which only the
`World` has, since it emits the [switch event](#uidallocator).

It holds three parts, each a borrow of its own:

| Part | Holds | Borrowed mutably by |
|---|---|---|
| Entity allocator | Generations, uids, cells, the uid map | `spawn`, `promote`, `adopt`, `despawn`, and uju when it sets a cell |
| Replica flags | A bit per entity index | `despawn`, and uju's runtime |
| Cell index | Per cell, its entities | `despawn`, and uju when it sets a cell |

So a query holding the replica flags or the cell index can still spawn and
promote inside its loop.

- **Entity to uid** is a slot per entity index, cleared on despawn, so a
  recycled index never inherits a uid.
- **Uid to entity** is a hash map. Every item a receiver applies goes
  through it, so it uses a fast integer hasher in place of SipHash. The
  hasher does not resist flooding: uids come from uju's own servers.
- **Despawn** frees the entity and forgets its uid. `resolve` on that uid
  returns `None` from then on.
