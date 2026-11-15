# Change tracking

How a table records what changed during a tick, and how a replicated table
turns that into the sections of a [cell frame](../protocol/frames.md#sections)
and applies the sections it receives. Storages in general are in
[Components](components.md#storage), and the `Changed` filter in
[Queries](queries.md#terms).

## Tracked tables

Two storages track changes:

| Storage | For | Replicated |
|---|---|---|
| `Tracked<C>` | Any component, such as a hand-written one that a system wants `Changed<C>` on | No |
| `ReplaceTable<C>` | A schema component sent as its whole value, `C: Replace` | Yes |

A component picks one as its storage:

```rust
#[derive(Component)]
#[component(storage = Tracked<Self>)]
struct Hp(u32);
```

Both lay values out as a [`SparseSet`](components.md#sparseset) does, which
they never hand out. Beside the dense array they keep, per entry, a **dirty
bit** and an **added bit**, and a **remove log** of entities.

| Method | Gives |
|---|---|
| `get_mut(entity)` | A `TrackedMut`, or `None` |
| `insert(entity, value)` | Inserts or replaces, marking the entry |
| `remove(entity)` | Removes, logging the entity |
| `is_changed(entity)` | Whether the entry was added or marked dirty this tick |
| `is_added(entity)` | Whether the entry was added this tick |
| `changed()` | The entities added or marked dirty this tick, in dense order |
| `removed()` | The entities removed this tick, in removal order |

They also offer `get`, `contains`, `len`, `is_empty`, `entities`, `iter`,
`components` and `clear`, as a `SparseSet` does. Mutable iteration goes
through a [query](queries.md) or [`join`](queries.md#join), whose items are
`TrackedMut`.

## Dirty and added bits

- **Marking on write.** Mutable access yields a `TrackedMut`, which derefs
  to `&C` and marks the entry dirty on `DerefMut`. Reading through a
  `Mut<C>` term marks nothing, so a system that only reads an entity does
  not resend it.
- **Insert.** Inserting over a value marks it dirty. A new value is marked
  added and dirty.
- **One bit per entry.** The bits are words beside the dense array, indexed
  by dense position. A removal swaps the last entry into the hole, and its
  bits move with it.
- **Shared words.** The `TrackedMut`s alive at once, such as a join's,
  share the words and set their bits through `&Cell<u64>`. That is a plain
  load and store, with no `unsafe`, since a `World` runs on one thread.
- **End of tick.** The `World` clears every table's bits and log when the
  tick ends.

A write that leaves the value as it was still marks it, and so does a
value changed and changed back within the tick. Nothing is compared.

## Removals and despawns

- **The remove log.** A removal logs the entity, and nothing else.
  [`commands().remove`](commands.md) logs as a direct removal does.
- **Add then remove.** Removing an entry whose added bit is set logs
  nothing, so the two cancel out.
- **Remove then add.** The log keeps the entity, and the new entry is
  added. Whoever reads the log skips an entity that holds the component
  again, and the new value is sent whole.
- **Despawns are not removals.** A despawn drops the entity's values from
  every table without logging them. The `World`'s
  [despawn log](entities.md#spawning-and-despawning) covers it.
- The log may hold entities that are dead by the end of the tick. Readers
  skip them.

## Replicas

A replica's state comes from its owner, so a `ReplaceTable` never marks or
logs it through the runtime's path:

- **Writes skip replicas.** A `Mut<C>` query term, a `ViewMut`'s `get_mut`
  and a `&mut view_mut` join term over a `ReplaceTable` skip replicas.
  Reads see them.
- **Once per entity.** A query with such a term is filtered as by
  [`Owned`](queries.md#terms), once whatever the number of such terms.
- **Received state is untracked.** Applying a section inserts, overwrites
  and removes values without marking or logging them.
- A `Tracked` table is not replicated, so writes reach replicas as for a
  `SparseSet`.

## Replicated components

A schema component implements `Replicated`, and a `replace` one also
`Replace`:

```rust
pub trait Replicated: Component {
    const WIRE_ID: u16;
    const VISIBILITY: Visibility;
    const HALO: bool = !matches!(Self::VISIBILITY, Visibility::None);
}

pub trait Replace: Replicated + Encode + for<'a> Decode<'a> {
    const SIZE: Option<usize>;
}
```

| Item | Meaning |
|---|---|
| `WIRE_ID` | The [component id](../protocol/wire-format.md#component-ids) |
| `VISIBILITY` | `Public`, `Owner`, `None`, or `Audience(id)` with the audience's [wire id](../protocol/wire-format.md#audience-ids) |
| `HALO` | Whether neighbor shards receive it. `true` unless `VISIBILITY` is `None` |
| `SIZE` | The size of every encoding when they all have one, `None` otherwise |
| `Encode`, `Decode` | The full encoding. `Decode` yields the owned value |

## The Table interface

The runtime drives every table through `Table`, without generics. A table
that is not replicated, such as a `SparseSet` or a `Tracked` one, reports
no `replication`, no change, writes no section, and panics if asked to
apply one.

| Method | Does |
|---|---|
| `replication()` | `Some(Replication { wire_id, visibility, halo })` for a replicated table |
| `changed(&mut out)` | Appends the entities added or marked dirty this tick |
| `removed()` | The remove log |
| `encode_section(full, changed, removed, &mut out)` | Appends one section, and returns whether it wrote one |
| `apply_section(buf, resolve)` | Applies one section, and returns its length |
| `end_tick()` | Clears the bits and the log |
| `despawn_batch(entities)` | Drops despawned entities' values without logging them |

### Sections

`encode_section` writes one component's section of a sub-frame, as
[Frames](../protocol/frames.md#sections) lays it out:

- **`full`** lists `(uid, entity)` pairs whose full value goes in the values
  part, such as the entities that entered the cell this tick.
- **`changed`** lists the pairs of entities reported by `changed`. A
  `replace` change is a full value too.
- **`removed`** lists the uids for the removed part: the logged entities
  that are alive and do not hold the component again.
- An entity must not be in both `full` and `changed`. Entities holding no
  value are skipped, and nothing is written when no part has items.
- **Snapshots** are sections with only `full`, listing every entity of the
  sub-frame.

A fixed-size component's values are packed `uid | value` items, so they
read as a slice of views. A variable-size one's are `uid | len | value`,
with a 1-byte `len` when `C::MAX_ENCODED_SIZE` is at most 255, and 2 bytes
otherwise.

`apply_section` reads the section after its component id: the `parts`
byte and the parts. For each item it calls `resolve(uid)`, which names the
entity to apply it to, or returns `None` to skip it. Values are inserted or
overwritten and removed uids removed, all untracked. A `replace` section
with a changes part, a truncated part or a zero incarnation is an error.

```rust
// on the sim, at the end of the tick, with `table: &dyn Table`
let mut changed = Vec::new();
table.changed(&mut changed);
// bucket by cell and sub-frame, then per bucket:
table.encode_section(&entered, &bucket, &removed, &mut frame)?;

// on the receiver, with `table: &mut dyn Table`
let read = table.apply_section(&section[2..], &mut |uid| world.resolve(uid))?;
```
