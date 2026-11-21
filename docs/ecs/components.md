# Components

How a type becomes a component or a unique of a `World`, how its values
are stored, and how components are inserted together. Entities are in [Entities](entities.md), iterating several
components together in [Queries](queries.md), and audience keys, which are
not components, in [Audiences](audiences.md).

## Component and Unique

A **component** is a value an entity holds, at most one per type per
entity. A **unique** is a value the `World` holds, at most one per type.

```rust
#[derive(Component)]
struct Hp(u32);

#[derive(Unique)]
struct Time { ticks: u64 }
```

- **Derived.** `#[derive(Component)]` and `#[derive(Unique)]` register the
  type. A generic type cannot derive either.
- **Ids.** `uju::init()` numbers registered types in order of their full
  path, so ids are dense from `0`. Call it before using any component or
  unique.
- **Bounds.** The only bound is `'static`. Neither trait requires `Send`:
  a `World` is built on its shard's thread and never leaves it, so a
  component or a unique can hold an `Rc` or another handle that is not
  `Send`.

## Access

| Method | Gives |
|---|---|
| `world.view::<C>()` | A [`View<C>`](#views): shared access to component `C`'s table |
| `world.view_mut::<C>()` | A [`ViewMut<C>`](#views): mutable access to component `C`'s table |
| `world.insert_unique(u)` | Inserts a unique, returning the previous value |
| `world.remove_unique::<U>()` | Removes a unique |
| `world.unique::<U>()`, `unique_mut::<U>()` | The unique. Panics when it is not inserted |
| `world.get_unique::<U>()`, `get_unique_mut::<U>()` | The unique, or `None` |

Each table and each unique is borrowed on its own, so views of different
components coexist. Borrowing one mutably while it is borrowed panics, as
with a `RefCell`.

## Storage

A component's table is its **storage**, the associated type
`Component::Storage`. It decides how values are laid out and what mutable
access hands out.

- **Derived.** `#[derive(Component)]` picks `SparseSet<C>`, unless the
  type names another with `#[component(storage = Tracked<Self>)]`. The
  same attribute takes [`require(...)`](#required-components).
- **Shared access** yields `&C`, whatever the storage.
- **Mutable access** yields the storage's `Mut<'a>` item. For a `SparseSet`
  it is `&'a mut C`. A storage that must see every write, such as one that
  tracks changes, makes it a wrapper instead, and [`Mut<C>`](queries.md#terms)
  query terms and [`join`](queries.md#join) yield that wrapper.

| Storage | Mutable item | Tracks changes | Replicated |
|---|---|---|---|
| `SparseSet<C>` | `&mut C` | No | No |
| [`Tracked<C>`](change-tracking.md#tracked-tables) | `TrackedMut<C>` | Yes | No |
| [`ReplaceTable<C>`](change-tracking.md#tracked-tables) | `TrackedMut<C>` | Yes | Yes, as whole values |
| [`PatchTable<C>`](change-tracking.md#field-masks) | `C::Mut`, which marks fields | Yes | Yes, as patches of the fields written |
| [`TagTable<C>`](change-tracking.md#tags) | `&C` | Yes | Yes, as additions and removals |
| [`OpsTable<C>`](change-tracking.md#ops) | `OpsMut<C>`, which applies ops | Yes | Yes, as ops |

uju's built-in `Transform` is stored in a `Tracked` table, so uju sets the
[cells](entities.md#setting-cells) of only the entities that moved.

### SparseSet

A `SparseSet<C>` keeps its values in a dense array, beside a dense array of
their entities, and maps an entity's index to its dense position through a
paged sparse array.

- **Lookup** by entity is a page read and a check of the dense entity, so a
  stale `Entity` from a despawned slot misses.
- **Removal** swaps the last value into the hole, so dense order is not
  insertion order and changes as values are removed.
- **Pages** of the sparse array hold 1024 indices each, and are allocated
  on first use and freed when empty.

### The Storage trait

A storage implements `Storage`, plus `Table`, the untyped interface the
`World` and the runtime use to drop a despawned entity's values, end the
tick and [replicate](change-tracking.md#the-table-interface), and
`Default`, which builds the empty table.

| Item | Meaning |
|---|---|
| `type Component` | The component it stores |
| `const REPLICATED` | Whether the component travels, so mutable access skips replicas. `false` by default |
| `type Mut<'a>` | One value borrowed mutably |
| `type JoinMut<'a>` | The mutable join term, built once per join or query from `&'a mut self` |
| `entities()` | The entities holding a value, in dense order |
| `get(entity)` | `Option<&C>` |
| `get_mut(entity)` | `Option<Mut<'_>>` |
| `insert(entity, value)` | Inserts or replaces, returning the previous value |
| `remove(entity)` | Removes, returning the value |
| `join_mut()` | The `JoinMut` |

`JoinMut` implements `Joinable`, an `unsafe` trait. Its `entities()` must
yield each entity at most once, and its `get(entity)` is called at most once
per entity for the join's whole lifetime, which is what lets it hand out
mutable items that live together. Building it once per join lets a storage
split its borrows there, such as handing every item a shared reference to
state they all update.

### Picking a storage

A hand-written `impl Component` picks any storage whose `Component` is the
type itself. It registers the type as the derive does, with the `Registration`
that numbers it:

```rust
use std::cell::UnsafeCell;
use uju::ecs::component::{COMPONENTS, Component, Id, Registration, new_table};

struct Score(i32);

#[uju::linkme::distributed_slice(COMPONENTS)]
#[linkme(crate = uju::linkme)]
static SCORE: Registration = Registration {
    name: concat!(module_path!(), "::Score"),
    id: UnsafeCell::new(u16::MAX),
    new_table: new_table::<Score>,
};

impl Component for Score {
    type Storage = Ledger<Score>; // a storage of your own

    fn id() -> Id {
        unsafe { *SCORE.id.get() }
    }
}
```

`name` must be unique among components, since ids follow it. A
hand-written `impl` declares its [required
components](#required-components) by overriding `required`.

## Views

A **view** is a borrow of one component's storage, checked out of the
`World`. `View<C>` derefs to `C::Storage`, and `ViewMut<C>` derefs to it
mutably, so a view offers the storage's own methods.

```rust
let mut hps = world.view_mut::<Hp>();
hps.insert(e, Hp(10));
hps.get_mut(e).unwrap().0 -= 3;
assert_eq!(hps.get(e), Some(&Hp(7)));
```

A `SparseSet` offers the following. Tracked storages offer the same but
`iter_mut` and `components_mut`, and a `TagTable` also but `components`,
plus their [changes](change-tracking.md#tracked-tables). An `OpsTable` adds
[`resync`, `get_mut_untracked` and `ops`](change-tracking.md#ops).

| Method | Does |
|---|---|
| `insert(entity, value)` | Inserts or replaces, returning the previous value |
| `remove(entity)` | Removes, returning the value |
| `get(entity)`, `get_mut(entity)` | The value, or `None` |
| `contains(entity)` | Whether the entity holds a value |
| `len()`, `is_empty()` | The number of values |
| `entities()` | The entities, in dense order |
| `iter()`, `iter_mut()` | `(Entity, &C)` or `(Entity, &mut C)`, in dense order |
| `components()`, `components_mut()` | The values alone, in dense order |
| `clear()` | Removes every value |

- **Dynamic borrows.** Each table sits in its own `RefCell`. Views of
  different components coexist, and checking out a view that conflicts with
  a live one panics.
- **Join terms.** `&view`, `&view_mut` and `&mut view_mut` are terms of
  [`join`](queries.md#join).
- **Deferred changes.** Inside a query's loop, a view of a table the query
  holds panics. [Commands](commands.md) insert and remove there instead.
- **Replicas.** When the storage is replicated, `view_mut.get_mut(entity)`
  returns `None` for a [replica](entities.md#replicas), and the
  `&mut view_mut` join term skips replicas. `insert` and `remove` do not.
- **One table.** A view's `insert` holds only its own table, so it adds no
  [required components](#required-components).

## Bundles

A **bundle** is components inserted together: a component, or a tuple of
up to 12 bundles.

```rust
let e = world.spawn_with((Position(pos), Hp(100), Hostile));
world.commands().insert(bag, (Position(pos), loot));
```

| Method | Does |
|---|---|
| `world.spawn_with(bundle)` | Spawns an entity holding the bundle, at once, and returns it |
| `world.commands().insert(entity, bundle)` | Inserts the bundle at the next [flush](commands.md#flush), replacing what the entity holds |

- **Each table once.** `spawn_with` borrows each table of the bundle once,
  so it panics when a query or a view holds one of them. Inside a query's
  loop, `spawn` then [`commands().insert`](commands.md) does the same,
  deferred.
- **Nesting.** A tuple's items may be tuples, so a function returning a
  bundle composes: `(enemy(), Loot(3))`.
- **Repeats.** A component that appears twice in a bundle is inserted
  twice, in order, so the last value wins: `(enemy(), Hp(50))` overrides
  the `Hp` that `enemy()` holds.
- **Required components** are added after the bundle, as below.

## Required components

A component may require others: an entity holding it must hold them too.
Each requirement names a component and its constructor.

```rust
#[derive(Component)]
#[component(require(Body = Body::new(), Mass))]
struct Collider(Shape);
```

- **Derived.** `require(Type = expr)` builds the value with `expr`, and
  `require(Type)` with `Default::default()`. The expression is evaluated
  only when the component is added.
- **Hand-written.** `Component::required` is empty by default. An `impl`
  overrides it:

  ```rust
  fn required(required: &mut Required<'_>) {
      required.add(Body::new);
      required.add(|| Mass(1.0));
  }
  ```

- **Missing only.** After a bundle insert, `spawn_with` or
  `commands().insert`, each requirement of each component of the bundle is
  added unless the entity holds that component, from the bundle or from
  before. Nothing is overwritten.
- **Transitive.** A component added as a requirement has its own
  requirements added in turn. They are added by depth, so the nearest
  requirement wins: when `Collider` requires `Mass` and `Body`, and `Body`
  requires another `Mass`, `Collider`'s is used. At equal depth, the first
  in bundle order, then in declaration order, wins.
- **Cycles** end by themselves: a component already held is not added
  again, so its requirements are not visited again.
- **View inserts** add none, since a view holds one table.
- **Replicas** get none, whatever inserts into them. Their schema
  components come from their owner, and the state the runtime receives
  never adds requirements.
- **Rust only.** Requirements are not a schema attribute, and nothing on
  the wire carries them.
- **Cost.** A component with no requirements costs nothing on the insert
  path. Each requirement borrows its table once per entity.

