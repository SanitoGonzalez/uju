use crate::runtime::node::Node;
use crate::runtime::shard::{self, Shard};
use crate::topology::{NodeId, ShardId};

pub struct Frame {
    pub node: NodeId,
    pub shard: ShardId,
    pub msg: Message,
    pub result_tx: Option<shard::FrameTx>,
}

impl Frame {
    pub fn with_current(msg: Message, result_tx: Option<shard::FrameTx>) -> Self {
        Self {
            node: Node::with(|n| n.id()),
            shard: Shard::with(|s| s.id()),
            msg,
            result_tx,
        }
    }
}

pub enum Message {}
