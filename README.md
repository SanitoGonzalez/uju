# uju

Experimental ideas for seamless MMO server

# 1. RUDP

RUDP is not a brand new idea, but would be essential for realtime actions which require low latency.

# 2. Multiversion Entity Component System

## Why?

Static scheduling of ordinary ECS (like [bevy_ecs](https://docs.rs/bevy_ecs/latest/bevy_ecs/)) serializes the systems
component-wide. (yet parallelizing disjoint components) Even if entity A and entity B is distinct, interaction to each
entities' component cannot be parallelized. This project designs an ECS interaction as a parallelized transaction,
keeping component storage/access as MVCC. (maybe hybrid - MVCC for )

### Key Points

- **Isolation Level / Write Skew**: SI with selective hardening, or SSI, or materialize-the-conflict (chosen per
  interaction type)
- **high contention on a component**: MVCC still cannot solve this
- **no side effect in transaction**: transactions must return an effect description the runtime performs exactly once
  after commit
- **ECS spawn/despawn**: how to handle this?

## 3. Server Meshing

- cluster > node > shard > cell
- *shard*: shard-per-core with shared-nothing model.
  (ref. [compio](https://compio.rs/), [seastar](https://seastar.io/shared-nothing/))

## Ideas & Keywords

- delta storage
- MVTO vs epoch-OCC (OCC for determinism?)
- tick-epoch-GC
- logical-indexed multiversion store

## To study

### Basics

- [ ] CMU Intro to Databse Systems: storage models, buffer pools, concurerncy control, isolation levels (skip SQL,
  recovery/logging, distributed for now <- durability is not a concern for this roject, since database is in memory)
- [ ] Kleppmann, Designing Data-Intensive Applications - chapter about transactions

### Implementation layer

- [ ] Petrov, Database Internals - part 1 only. part 2 about disbrituted systems for future work

### Papers

- [ ] Wu et al., "An Empirical Evaluation of In-Memory MVCC" (VLDB 2017) : a menu for design decisions
- [ ] HyPer MVCC (Neumann et al., SIGMOD 2015) : delta storage over a column store.
- [ ] Silo (Tu et al., SOSP 2013) : epoch-based concurrency, no global clock. timing model
- [ ] Hekaton (Larson et al., VLDB 2011) :  lock-free in-memory MVCC, logical indexing

# X. Idea Notes

- [spatial network](https://en.wikipedia.org/wiki/Spatial_network):
    - shard worlds of ECS, letting each thread run the shard
    - use position as a shard criteria
    - migrate entities on shard criteria
    - migrate entities if cross-shard interaction exceeds the threshold

- MVCC (2026.09.06):
    - Postgres-styled MVCC looks to be a good fit here, though it's considered "terrible backend" on disk oriented DBMS
      field.
    - Protocol = MV-2PL/MV-TO: On write-write, let the last writer wait. (Must not block the thread, though.) Abortion
      on write conflict requires retriable transaction, which is not easy on game programming. So we better wait rather
      than abort. However, This requires EVERY yield in the transaction to have a reasonable timeout to prevent HOLB.
      (Inter-shard transaction will yield. If the peer shard dies without responding, it will be an infinite stall if no
      timeout.)
    - Version Storage = Append-Only Storage: All versions should reside in the one dense array. If we manage separate
      arena, the iteration would lack cache locality. Therefore, keep version chains in the dense array and let
      iteration see the latest version of its timestamp view. Delta Storage might introduce the burden for the users to
      implement and manage delta ops per component.
    - Garbage Collection: Transaction level
    - Indexes: Only implement/provide primary index as entity. Secondary indexes introduce complexity and most game
      logics don't need them.
    