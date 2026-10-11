use crate::cluster::node::{self, Node};
use crate::cluster::shard::{self, Shard};

pub struct Frame {
    pub node: node::Id,
    pub shard: shard::Id,
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
