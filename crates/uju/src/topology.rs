//! The vocabulary of a uju deployment's layout, shared by every role. See
//! `docs/architectures/topology.md`.

/// A node: one process, running one pinned thread per allowed CPU.
pub type NodeId = u16;

/// A shard: one pinned thread on a sim or hub node, unique within its node.
pub type ShardId = u16;
