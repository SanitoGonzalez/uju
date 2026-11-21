# Queries

How to iterate the entities that hold several components at once. A
`Query` checks out the tables it needs from the `World`. [`join`](#join)
does the same over [views](components.md#views) already checked out.

## Query

A `Query<Q>` names its terms in its type. `Query::new(&world)` borrows each
term's table, and `iter()` walks the entities that match every term.

```rust
let mut query = Query::<(Mut<Position>, Velocity, Without<Frozen>)>::new(&world);
for (entity, (position, velocity)) in query.iter() {
    position.0 += velocity.0;
}
```

- **Borrows.** The query holds its tables' borrows until it is dropped, so
  it can be iterated again without checking them out anew. A term that
  conflicts with a live borrow of the same table, from another query, a
  view or the query itself, panics in `Query::new`.
- **Items.** `iter()` yields `(Entity, item)`. The item is a tuple of the
  data terms' items, with filters left out. A query with one data term
  yields that item bare.
- **Order.** Entities come in the dense order of the
  [driving](#driving) table, which removals change, or in index order when
  no term drives. No order is promised.

## Terms

| Term | Matches entities | Yields | Borrows `C`'s table |
|---|---|---|---|
| `C` | Holding `C` | `&C` | Shared |
| `Mut<C>` | Holding `C`, owned when `C`'s storage is replicated | `C::Storage`'s [`Mut`](components.md#storage) item, `&mut C` for a `SparseSet` | Mutably |
| `With<C>` | Holding `C` | Nothing | Shared |
| `Without<C>` | Not holding `C` | Nothing | Shared |
| `Option<C>` | Every entity | `Option<&C>`, `None` without `C` | Shared |
| `Option<Mut<C>>` | Every entity, owned when `C`'s storage is replicated | `Option` of the `Mut` item | Mutably |
| `Has<C>` | Every entity | `bool`, whether it holds `C` | Shared |
| `Changed<C>` | Whose `C` was added or [marked dirty](change-tracking.md#dirty-and-added-bits) this tick | Nothing | Shared |
| `Added<C>` | That gained `C` this tick | Nothing | Shared |
| `Owned` | Owned by this `World` | Nothing | None. Borrows the replica flags shared |
| `Replica` | [Replicas](entities.md#replicas) | Nothing | None. Borrows the replica flags shared |
| `Or<(F0, F1, ...)>` | Matching any of the filters | Nothing | As its filters do |
| `(T0, T1, ...)` | Matching every term | The terms' items | As its terms do |

- **Tuples** hold up to 12 terms and nest. Nested tuples flatten into one
  item: `(A, B, (With<F>, Without<G>))` yields `(&A, &B)`, and
  `(Mut<A>, With<F>)` yields the `Mut` item alone.
- **Mutable terms stand alone.** `Mut<C>` and `Option<Mut<C>>` conflict
  with any other term over `C`, such as `With<C>`, and panic. `Mut<C>`
  already requires `C`.
- **Filters.** `With`, `Without`, `Changed`, `Added`, `Owned`, `Replica`,
  `Or` and tuples of them are filters, the `Filter` trait. `Or` takes
  filters only, and a tuple inside it matches when all of its filters do:
  `Or<((With<A>, With<B>), Changed<C>)>`. To test for `C` inside an `Or`,
  use `With<C>`.
- **`Owned` and `Replica` leave spawning free.** The replica flags are a
  borrow of their own, so a loop over such a query can still spawn.
- **Writes skip replicas.** A query with a `Mut<C>` or `Option<Mut<C>>`
  term over a replicated storage, such as a `ReplaceTable`, is filtered as
  by `Owned`. So `Option<Mut<C>>` never yields `None` for a replica that
  holds `C`: the replica is not yielded at all. The flag is read once per
  entity, however many such terms the query has.
- **`Changed<C>` and `Added<C>` need a tracked storage.** Over a
  `SparseSet`, the query does not compile. An added value is marked dirty
  too, so it matches both. See
  [Change tracking](change-tracking.md#change-filters).

```rust
let mut q = Query::<(Mut<Position>, Option<Velocity>, Has<Frozen>)>::new(&world);
for (_, (position, velocity, frozen)) in q.iter() {
    if let Some(velocity) = velocity && !frozen {
        position.0 += velocity.0;
    }
}

let mut q = Query::<(Hp, Or<(Added<Hp>, With<Poisoned>)>)>::new(&world);
```

## Driving

A join walks the entities of one term, its **driving** term, and probes
every other term for each of them. The driver is the term with the fewest
entities, so a query over a large table and a small one costs the small
one's size.

- `C`, `Mut<C>`, `With<C>`, `Changed<C>` and `Added<C>` count their
  table's entities. So a `With<C>` over a small table drives a query over
  large ones.
- `Without<C>` lists the entities it excludes, so it never drives.
  `Option`, `Has`, `Or`, `Owned` and `Replica` match entities outside any
  one table, so they never drive either.
- A tuple drives with its smallest term.
- In [`iter_in`](#iter_in), the cell's list is one more term.
- **No driving term.** A query made only of terms that never drive, such
  as `Query::<Without<Frozen>>` or `Query::<(Option<A>, Has<B>)>`, walks
  every live entity. `iter()` lists them into a buffer the query keeps, so
  the loop does not hold the entity allocator and can spawn. Entities
  spawned in the loop are not visited. Listing costs a pass over every
  entity index.

## iter_in

A cell is a value, not a type, so it is an argument of the iteration rather
than a term. `query.iter_in(cell)` walks the entities of the query that are
in `cell`:

```rust
let mut q = Query::<(Mut<Hp>, Pos, Owned)>::new(&world);
for cell in cells {
    for (e, (hp, pos)) in q.iter_in(cell) { ... }
}
```

- **One more term.** The cell's list from the
  [cell index](entities.md#cell-index) joins as a term that matches the
  entities in the cell. It drives when it is the shortest, so a query over
  large tables costs the cell's size.
- **No repeats.** An entity is in one cell only, so iterating several cells
  in turn never yields an entity twice.
- **Filters combine.** `Owned` and `Replica` filter it as any query.
  Replicas are in the index, so without a filter `iter_in` yields both.
- **Borrows.** The first `iter_in` borrows the cell index shared, until the
  query is dropped. Spawning stays free inside the loop. A cell with no
  entity yields nothing.

## get

`query.get(entity)` yields one entity's item, as `iter()` would, or `None`
when a term rejects it:

```rust
let mut q = Query::<(Mut<Hp>, Without<Shield>)>::new(&world);
if let Some(mut hp) = q.get(target) {
    hp.0 -= damage;
}
```

- **One at a time.** The item borrows the query mutably, so it is dropped
  before the next `get` or `iter`. Calling `get` again for the same entity
  is fine.
- **Same rules.** Replicas are skipped as in `iter()`. When no term drives,
  a dead entity yields `None`.
- **Cost.** A probe per term, no iteration.

## join

`join(terms)` iterates views or storages already checked out, and yields
`(Entity, items)`. Its terms are:

| Term | Yields |
|---|---|
| `&view`, `&view_mut` | `&C` |
| `&mut view_mut` | The storage's `Mut` item. Skips replicas when the storage is replicated |
| `&storage`, `&mut storage` | The same, over a storage held directly |
| `(T0, T1, ...)` | A tuple of the terms' items, up to 12 |

```rust
let mut positions = world.view_mut::<Position>();
let velocities = world.view::<Velocity>();
for (entity, (position, velocity)) in join((&mut positions, &velocities)) {
    position.0 += velocity.0;
}
```

- **Tuples always.** `join` yields a tuple even for one term, and does not
  flatten nested tuples. Filters are `Query` terms only.
- **Driving** follows the same rule as a query's.
- **Views stay usable.** The views outlive the join, so a system can read
  or insert through them between joins.
