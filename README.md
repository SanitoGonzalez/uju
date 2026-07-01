# 우주 / Uju

Experimental ideas for seamless MMO server

# 1. RUDP

RUDP is not a brand new idea, but would be essential for realtime actions which require low latency.

# 2. Multiversion Entity Component System

## Why?

Static scheduling of ordinary ECS (like [bevy_ecs](https://docs.rs/bevy_ecs/latest/bevy_ecs/)) serializes the systems component-wide. (yet parallelizing disjoint components) Even if entity A and entity B is distinct, interaction to each entities' component cannot be parallelized.
This project designs an ECS interaction as a parallelized transaction, keeping component storage/access as MVCC. (maybe hybrid - MVCC for )

### Key Points
- **Isolation Level / Write Skew**: SI with selective hardening, or SSI, or materialize-the-conflict (chosen per interaction type)
- **high contention on a component**: MVCC still cannot solve this
- **no side effect in transaction**: transactions must return an effect description the runtime performs exactly once after commit
- **ECS spawn/despawn**: how to handle this?


## Ideas & Keywords

- delta storage
- MVTO vs epoch-OCC (OCC for determinism?)
- tick-epoch-GC
- logical-indexed multiversion store

## To study

### Basics
- [ ] CMU Intro to Databse Systems: storage models, buffer pools, concurerncy control, isolation levels (skip SQL, recovery/logging, distributed for now <- durability is not a concern for this roject, since database is in memory)
- [ ] Kleppmann, Designing Data-Intensive Applications - chapter about transactions

### Implementation layer
- [ ] Petrov, Database Internals - part 1 only. part 2 about disbrituted systems for future work

### Papers
- [ ] Wu et al., "An Empirical Evaluation of In-Memory MVCC" (VLDB 2017) : a menu for design decisions
- [ ] HyPer MVCC (Neumann et al., SIGMOD 2015) : delta storage over a column store.
- [ ] Silo (Tu et al., SOSP 2013) : epoch-based concurrency, no global clock. timing model
- [ ] Hekaton (Larson et al., VLDB 2011) :  lock-free in-memory MVCC, logical indexing
