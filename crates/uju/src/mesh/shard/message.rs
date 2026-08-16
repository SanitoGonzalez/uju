use smallvec::SmallVec;

use crate::ecs::entity::UniversalEntity;
use crate::mesh::{node, shard};

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

pub enum Message {
    Replication(Vec<ReplicationOp>),
}

pub enum ReplicationOp {
    Upsert {
        universal: UniversalEntity,
        components: SmallVec<
            [(
                i32, /*todo: component id*/
                i32, /*todo: erased component*/
            ); 4],
        >, // todo: erased component might be: `Box<dyn Any + Send>`
    },
    Remove {
        universal: UniversalEntity,
    }, // means "despawned at origin" or "left your AOI" -> unlink and despawn
}
