use crate::shard::{self, Shard};

pub struct Node {
    shards: Vec<Shard>,
}

impl Node {
    pub fn run(&self) {}
}

#[derive(Debug, Clone)]
pub struct Builder {
    shard_builder: shard::Builder,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Builder {
    pub fn new() -> Self {
        Self {
            shard_builder: shard::Builder::new(),
        }
    }

    pub fn with_shard(&mut self, builder: shard::Builder) -> &mut Self {
        self.shard_builder = builder;
        self
    }

    pub fn build(&self) -> std::io::Result<Node> {
        // todo: support NUMA
        let shards_total = std::thread::available_parallelism()?.get();
        let mut shards = Vec::with_capacity(shards_total);

        let mut senders = Vec::with_capacity(shards_total);
        let mut receivers = Vec::with_capacity(shards_total);
        for _ in 0..shards_total {
            let (tx, rx) = crossfire::mpsc::bounded_blocking_async(16);
            senders.push(tx);
            receivers.push(Some(rx));
        }

        for shard_id in 0..shards_total {
            // todo: must build shard within the shard thread
            shards.push(self.shard_builder.build(
                shard_id as shard::Id,
                senders.clone(),
                receivers[shard_id].take().unwrap(),
            )?);
        }

        Ok(Node { shards })
    }
}
