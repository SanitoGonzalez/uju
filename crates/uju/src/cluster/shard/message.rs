use crate::cluster::{node, shard};

pub struct Frame {
    pub node: node::Id,
    pub shard: shard::Id,
    pub msg: Message,
    pub result_tx: Option<shard::FrameTx>,
}

impl Frame {
    pub fn with_current(msg: Message, result_tx: Option<shard::FrameTx>) -> Self {
        Self {
            node: node::current(),
            shard: shard::current(),
            msg,
            result_tx,
        }
    }
}

pub enum Message {}
