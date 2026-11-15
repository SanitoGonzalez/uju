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
  [driving](#driving) table, which removals change. No order is promised.

## Terms

| Term | Matches entities | Yields | Borrows `C`'s table |
|---|---|---|---|
| `C` | Holding `C` | `&C` | Shared |
| `Mut<C>` | Holding `C` | `C::Storage`'s [`Mut`](components.md#storage) item, `&mut C` for a `SparseSet` | Mutably |
| `With<C>` | Holding `C` | Nothing | Shared |
| `Without<C>` | Not holding `C` | Nothing | Shared |
| `Owned` | Owned by this `World` | Nothing | None. Borrows the replica flags shared |
| `Replica` | [Replicas](entities.md#replicas) | Nothing | None. Borrows the replica flags shared |
| `(T0, T1, ...)` | Matching every term | The terms' items | As its terms do |

- **Tuples** hold up to 12 terms and nest. Nested tuples flatten into one
  item: `(A, B, (With<F>, Without<G>))` yields `(&A, &B)`, and
  `(Mut<A>, With<F>)` yields the `Mut` item alone.
- **Mutable terms stand alone.** `Mut<C>` conflicts with any other term
  over `C`, such as `With<C>`, and panics. `Mut<C>` already requires `C`.
- **Filters alone.** `Without`, `Owned` and `Replica` cannot drive
  iteration, so `iter()` panics on a query made only of them. `iter_in`
  accepts one, since the cell drives.
- **`Owned` and `Replica` leave spawning free.** The replica flags are a
  borrow of their own, so a loop over such a query can still spawn.

## Driving

A join walks the entities of one term, its **driving** term, and probes
every other term for each of them. The driver is the term with the fewest
entities, so a query over a large table and a small one costs the small
one's size.

- `C`, `Mut<C>` and `With<C>` count their table's entities. So a `With<C>`
  over a small table drives a query over large ones.
- `Without<C>` lists the entities it excludes, so it never drives.
  `Owned` and `Replica` list none, so they never drive either.
- A tuple drives with its smallest term.
- In [`iter_in`](#iter_in), the cell's list is one more term.

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

## join

`join(terms)` iterates views or storages already checked out, and yields
`(Entity, items)`. Its terms are:

| Term | Yields |
|---|---|
| `&view`, `&view_mut` | `&C` |
| `&mut view_mut` | The storage's `Mut` item |
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
