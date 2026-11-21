# Change tracking

How a table records what changed during a tick, and how a replicated table
turns that into the sections of a [cell frame](../protocol/frames.md#sections)
and applies the sections it receives. Storages in general are in
[Components](components.md#storage), query terms in
[Queries](queries.md#terms), and the tracking of audience keys in
[Audiences](audiences.md#tracking).

## Tracked tables

Five storages track changes:

| Storage | For | Mutable item | Replicated |
|---|---|---|---|
| `Tracked<C>` | Any component, such as a hand-written one that a system wants [`Changed<C>`](#change-filters) on | `TrackedMut<C>` | No |
| `ReplaceTable<C>` | A schema component sent as its whole value, `C: Replace` | `TrackedMut<C>` | Yes |
| `PatchTable<C>` | A schema component sent as patches of its fields, `C: Patch` | `C::Mut` | Yes |
| `TagTable<C>` | A [tag](#tags), `C: Replicated + Default` | `&C` | Yes |
| `OpsTable<C>` | A schema component changed only through [ops](#ops), `C: Ops` | `OpsMut<C>` | Yes |

A component picks one as its storage:

```rust
#[derive(Component)]
#[component(storage = Tracked<Self>)]
struct Hp(u32);
```

All five lay values out as a [`SparseSet`](components.md#sparseset) does,
which they never hand out. Beside the dense array they keep, per entry, a
**dirty bit** and an **added bit**, and a **remove log** of entities. A
`PatchTable` also keeps a [field mask](#field-masks) per entry, and an
`OpsTable` the [tick's ops](#the-arena).

| Method | Gives |
|---|---|
| `get_mut(entity)` | The mutable item, or `None` |
| `insert(entity, value)` | Inserts or replaces, marking the entry |
| `remove(entity)` | Removes, logging the entity |
| `is_changed(entity)` | Whether the entry was added or marked dirty this tick |
| `is_added(entity)` | Whether the entry was added this tick |
| `changed()` | The entities added or marked dirty this tick, in dense order |
| `removed()` | The entities removed this tick, in removal order |

A `TagTable` has `added()` in place of `is_changed` and `changed()`, since
a tag is never marked dirty. They also offer `get`, `contains`, `len`,
`is_empty`, `entities`, `iter` and `clear`, as a `SparseSet` does, and all
but `TagTable` offer `components`.
Mutable iteration goes through a [query](queries.md) or
[`join`](queries.md#join), whose items are the mutable items.

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
- **End of tick.** The `World` clears every table's bits, masks and log
  when the tick ends.

A write that leaves the value as it was still marks it, and so does a
value changed and changed back within the tick. Nothing is compared.

## Change filters

Systems read the tick's changes through two query filters and a `World`
method:

| Reads | Gives | From |
|---|---|---|
| `Changed<C>` | The entities whose `C` was added or marked dirty this tick | The dirty bits |
| `Added<C>` | The entities that gained `C` this tick | The added bits |
| `world.removed::<C>()` | The entities that lost `C` this tick and are still alive | The remove log |

```rust
let mut q = Query::<(Hp, Added<Hp>)>::new(&world);
for (e, hp) in q.iter() { /* first tick with an Hp */ }

for e in world.removed::<Hp>() {
    world.commands().insert(e, Downed);
}
```

- **Adds are changes.** An added value is marked dirty too, so it matches
  both `Changed<C>` and `Added<C>`. Both combine with other
  [filters](queries.md#terms), inside `Or` too, such as
  `Or<(Added<Hp>, Changed<Armor>)>`.
- **Tracked tables only.** All three need `C`'s storage to be one of the
  [tracked tables](#tracked-tables). Over a `SparseSet`, they do not
  compile. A hand-written component picks `Tracked<Self>` to get them.
- **`removed` is not a query term**, since its entities no longer hold
  `C`. It yields them in removal order and skips, from the
  [remove log](#removals-and-despawns), an entity that is dead or holds `C`
  again: a remove then an add shows as `Added<C>` only.
- **Despawns do not show.** A despawn writes nothing to a table's log, so
  neither `removed` nor any filter sees it. [Component
  hooks](components.md#hooks) see despawns.
- **Borrows.** `removed` holds `C`'s table shared while its iterator
  lives, and checks liveness per entity, so its loop can spawn and queue
  [commands](commands.md), but not take `C`'s table mutably.
- **Received state does not show.** Applying a received section marks
  and logs nothing, and writes to a replicated table
  [skip replicas](#replicas), so for such a table the filters and
  `removed` cover the entities this world owns.

## Field masks

A `PatchTable` tracks changes per field, so a change sends only the
fields written:

- **One mask per entry.** Beside the dense array, each entry has
  `C::FIELDS.div_ceil(8)` bytes, a bit per top-level field in declaration
  order. `C` stays a plain struct, and entities without it pay nothing.
- **Marking on write.** Mutable access yields `C::Mut`, a wrapper with a
  `*_mut()` accessor per field, which marks the field and the entry dirty,
  and a read accessor, which marks nothing. `as_mut_all()` hands out
  `&mut C` and marks every field.

  ```rust
  let mut stats = view.get_mut(entity).unwrap();   // StatsMut<'_>
  *stats.hp_mut() -= 10;                            // marks hp
  let hp = stats.hp();                              // marks nothing
  stats.as_mut_all().take_damage(5);                // marks every field
  ```

- **Insert.** Inserting over a value marks every field. A new value is
  marked added and dirty, and its mask stays clear, since it is sent whole.
- **Moves.** A removal swaps the last entry into the hole, and its mask
  moves with its bits.
- **End of tick.** The masks of the entries marked dirty are cleared with
  the bits.
- `mask(entity)` reads an entry's mask.

A field whose accessor was called is sent even if its value ends up
unchanged. Nothing is compared or cloned.

## Tags

A tag has no value: only its addition and removal travel. A `TagTable`
holds unit structs, which take no memory, so it stores only membership.

- **Insert.** Adding a tag marks the entry added. Inserting one the entity
  holds changes nothing and marks nothing, so `Changed<C>` keeps the
  entities that gained the tag this tick.
- **No writes.** The mutable item is `&C`, as there is nothing to write. A
  `Mut<C>` term reads, and skips replicas as for any replicated table.
- **Queries.** A tag is a component, so `C`, `With<C>`, `Without<C>`,
  `Changed<C>` and `Added<C>` terms match it, and `world.removed::<C>()`
  lists the entities that lost it.
- **Receiving.** The receiver builds the tag with `Default`.

## Ops

An `OpsTable` holds a component that changes only through ops, so that
every change reaches replicas. It hands out no `&mut C`.

- **Ops are facts.** An op is what the sim has decided, not a request, so
  whatever could refuse it is checked before the op is built. `Ops::apply`
  must be total and deterministic, so every receiver reaches the sim's
  value.
- **Applying.** Mutable access yields an `OpsMut<C>`, which derefs to `&C`
  and has `apply(op)`. It calls `Ops::apply` on the value, records the op
  and marks the entry dirty.
- **One entity.** `world.apply_op::<C>(entity, op)` applies one op. It
  returns `false` when the entity holds no `C` or is a replica, and panics
  like any conflicting borrow while a query holds the table.

```rust
let mut q = Query::<(Mut<Inventory>, Pickup)>::new(&world);
for (_, (mut inv, pickup)) in q.iter() {
    if inv.has_room() {
        let slot = inv.free_slot();
        inv.apply(InventoryOp::Add { item: pickup.0, slot });
    }
}

world.apply_op::<Inventory>(entity, InventoryOp::Clear);
```

| Method | Does |
|---|---|
| `get_mut(entity)` | The `OpsMut`, or `None` |
| `insert(entity, value)` | Inserts or replaces. A replaced value is resynced |
| `resync(entity)` | Sends the full value in the next delta, dropping the tick's ops |
| `get_mut_untracked(entity)` | `&mut C`, recording nothing |
| `ops(entity)` | The entity's ops recorded this tick, in order |

- **Sent whole.** An entry added, inserted over or resynced this tick is
  sent as its full value. It records no op for the rest of the tick, since
  the value sent holds them, and the ops it had are dropped.
- **Untracked writes.** `get_mut_untracked` is safe, but replicas miss what
  it writes until a resync. A replica diverging is a logic error, not
  undefined behavior, so its name warns rather than `unsafe`.
- **Resync.** `world.resync::<C>(entity)` resyncs an owned entity, and
  returns `false` for a replica or an entity without `C`.

```rust
let mut inventories = world.view_mut::<Inventory>();
inventories.get_mut_untracked(entity).unwrap().items.clear();
drop(inventories);
world.resync::<Inventory>(entity);
```

### The arena

- **One arena per table.** The tick's ops sit in one `Vec<(C::Op, u32)>`,
  each op beside the position of its entity's next op.
- **8 bytes per entry.** Each entry keeps the `u32` positions of its first
  and last op. Recording appends to the arena and links the entry's last
  op to it, so no entity owns a heap block.
- **In order.** The encoder follows an entry's links, so it reads the
  entity's ops in the order applied, without sorting.
- **Moves.** A removal swaps the last entry into the hole, and its
  positions move with its bits. A resync or a removal resets them.
- **End of tick.** One `clear()` empties the arena, and the positions of
  the entries marked dirty are reset with the bits.

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

A replica's state comes from its owner, so a replicated table, a
`ReplaceTable`, a `PatchTable`, a `TagTable` or an `OpsTable`, never marks
or logs it through the runtime's path:

- **Writes skip replicas.** A `Mut<C>` query term, a `ViewMut`'s `get_mut`
  and a `&mut view_mut` join term over a replicated table skip replicas,
  and so do `world.apply_op` and `world.resync`. Reads see them.
- **Once per entity.** A query with such a term is filtered as by
  [`Owned`](queries.md#terms), once whatever the number of such terms.
- **Received state is untracked.** Applying a section inserts, overwrites
  and removes values without marking or logging them.
- A `Tracked` table is not replicated, so writes reach replicas as for a
  `SparseSet`.

## Replicated components

Every schema component and tag implements `Replicated`. A component with a
value also implements `Valued`, its full encoding, and its mode's trait,
`Replace`, `Patch` or `Ops`. A tag implements `Replicated` alone.

```rust
pub trait Replicated: Component {
    const WIRE_ID: u16;
    const VISIBILITY: Visibility;
    const HALO: bool = !matches!(Self::VISIBILITY, Visibility::None);
}

pub trait Valued: Replicated + Encode + for<'a> Decode<'a> {
    const SIZE: Option<usize>;
}

pub trait Replace: Valued {}

pub trait Patch: Valued {
    const FIELDS: usize;
    const MAX_PATCH_SIZE: Option<usize>;
    type Mut<'a>;
    type Patch<'a>: Decode<'a>;

    fn new_mut<'a>(value: &'a mut Self, mask: FieldMask<'a, Self>) -> Self::Mut<'a>;
    fn patch_size(&self, mask: &[u8]) -> usize;
    fn encode_patch(&self, mask: &[u8], buf: &mut [u8]) -> Result<usize, EncodeError>;
    fn apply(&mut self, patch: Self::Patch<'_>);
}

pub trait Ops: Valued {
    type Op: Encode + for<'a> Decode<'a> + 'static;

    fn apply(&mut self, op: &Self::Op);
}
```

| Item | Meaning |
|---|---|
| `WIRE_ID` | The [component id](../protocol/wire-format.md#component-ids) |
| `VISIBILITY` | `Public`, `Owner`, `None`, or `Audience(id)` with the [audience](audiences.md)'s [wire id](../protocol/wire-format.md#audience-ids) |
| `HALO` | Whether neighbor shards receive it. `true` unless `VISIBILITY` is `None` |
| `SIZE` | The size of every full encoding when they all have one, `None` otherwise |
| `Encode`, `Decode` | The full encoding. `Decode` yields the owned value |
| `FIELDS` | The number of top-level fields, a bit each in the mask |
| `MAX_PATCH_SIZE` | The largest [patch](../protocol/wire-format.md#patches), `None` when only the message cap bounds it |
| `Mut<'a>` | The wrapper mutable access yields |
| `Patch<'a>` | A received patch, validated by `Decode` |
| `new_mut` | Wraps an entry's value and its `FieldMask` |
| `patch_size`, `encode_patch` | The patch of the fields set in `mask`, from the current value. The mask is the patch's dirty bits, as is |
| `Patch::apply` | Overwrites the fields the patch holds |
| `Op` | The op type, a schema [union](../protocol/wire-format.md#unions) |
| `Ops::apply` | Applies an op. Total and deterministic |

### Writing a Patch

protoc generates these impls for a `@replicate(patch)` component. The
wrapper holds the value and the entry's `FieldMask`, and each `*_mut()`
accessor calls `mark` with the field's index before handing the field
out:

```rust
pub struct StatsMut<'a> {
    value: &'a mut Stats,
    mask: FieldMask<'a, Stats>,
}

impl StatsMut<'_> {
    pub fn hp(&self) -> u16 {
        self.value.hp
    }

    pub fn hp_mut(&mut self) -> &mut u16 {
        self.mask.mark(1);
        &mut self.value.hp
    }

    pub fn as_mut_all(&mut self) -> &mut Stats {
        self.mask.mark_all();
        self.value
    }
}

impl Patch for Stats {
    type Mut<'a> = StatsMut<'a>;

    fn new_mut<'a>(value: &'a mut Self, mask: FieldMask<'a, Self>) -> StatsMut<'a> {
        StatsMut { value, mask }
    }
    // ...
}
```

`FieldMask` sets its bits through `Cell`s, so the wrappers alive at once,
such as a join's, share the table's masks with no `unsafe`, as
`TrackedMut`s share the dirty bits. The mask's bits past `FIELDS` stay
zero, so `encode_patch` copies it in as the patch's dirty bits and adds
the bits that follow them.

### Writing Ops

`Ops::apply` gives an op its meaning, the same on the sim and on every
receiver. The op type is a plain enum with its encoding:

```rust
impl Ops for Inventory {
    type Op = InventoryOp;

    fn apply(&mut self, op: &InventoryOp) {
        match *op {
            InventoryOp::Add { item, slot } => {
                self.items.insert(slot, item);
            }
            InventoryOp::Remove(slot) => {
                self.items.remove(&slot);
            }
            InventoryOp::Clear => self.items.clear(),
        }
    }
}
```

Every op has a result, even one that does not fit the value, such as
removing from an empty slot. A system that must not add to a full slot
checks before it builds the `Add`.

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
| `despawn_batch(entities)` | Drops despawned entities' values without logging them in the remove log. [`on_remove` hooks](components.md#hooks) still get them |

### Sections

`encode_section` writes one component's section of a sub-frame, as
[Frames](../protocol/frames.md#sections) lays it out:

- **`full`** lists `(uid, entity)` pairs whose full value goes in the values
  part, such as the entities that entered the cell this tick.
- **`changed`** lists the pairs of entities reported by `changed`. A
  `replace` change is a full value too. A `patch` change is a full value
  when the entry was added this tick, and a patch of its marked fields
  otherwise. An `ops` change is a full value when the entry was added,
  inserted over or resynced this tick, and its ops otherwise. A tag's is
  an addition.
- **Overflow.** When an entity's ops would not fit the item's `u16` `len`,
  its full value goes in the values part instead. The value's size is
  never computed to compare.
- **`removed`** lists the uids for the removed part: the logged entities
  that are alive and do not hold the component again.
- An entity must not be in both `full` and `changed`. Entities holding no
  value are skipped, and nothing is written when no part has items.
- **Snapshots** are sections with only `full`, listing every entity of the
  sub-frame.

| Table | Values part | Changes part | Removed part |
|---|---|---|---|
| `ReplaceTable` | `uid \| value` | None | `uid` |
| `PatchTable` | `uid \| value` | `uid \| len \| patch` | `uid` |
| `TagTable` | `uid` | None | `uid` |
| `OpsTable` | `uid \| len \| value` | `uid \| len:u16 \| ops` | `uid` |

A fixed-size component's values are packed `uid | value` items, so they
read as a slice of views. A variable-size one's are `uid | len | value`,
with a 1-byte `len` when `C::MAX_ENCODED_SIZE` is at most 255, and 2 bytes
otherwise. A patch always has a `len`, 1 byte when `C::MAX_PATCH_SIZE` is
at most 255, and 2 otherwise. An `ops` item's `len` is 2 bytes, and `ops`
is a `vec` of `C::Op` with no `max_length`: a 2-byte count, then each op
behind a length prefix but the last, 1 byte when `C::Op::MAX_ENCODED_SIZE`
is at most 255, and 2 otherwise.

`apply_section` reads the section after its component id: the `parts`
byte and the parts. For each item it calls `resolve(uid)`, which names the
entity to apply it to, or returns `None` to skip it. Values are inserted or
overwritten, patches applied with `Patch::apply`, ops with `Ops::apply` in
order, tags added with `Default`, and removed uids removed, all untracked.
A patch or ops to an entity that holds no value are skipped. A part the
table does not write, a truncated part, an invalid value, patch or op, or a
zero incarnation is an error.

```rust
// on the sim, at the end of the tick, with `table: &dyn Table`
let mut changed = Vec::new();
table.changed(&mut changed);
// bucket by cell and sub-frame, then per bucket:
table.encode_section(&entered, &bucket, &removed, &mut frame)?;

// on the receiver, with `table: &mut dyn Table`
let read = table.apply_section(&section[2..], &mut |uid| world.resolve(uid))?;
```
