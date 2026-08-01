use std::thread::JoinHandle;

use tracing::{error, info, warn};

use crate::shard;

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

    pub fn with_shard(mut self, builder: shard::Builder) -> Self {
        self.shard_builder = builder;
        self
    }

    pub fn run(self) -> std::io::Result<()> {
        let runtime = compio::runtime::Runtime::new()?;
        runtime.block_on(async move {
            self.run_inner().await?;
            Ok(())
        })
    }

    async fn run_inner(self) -> std::io::Result<()> {
        // todo: support NUMA
        let shards_total = std::thread::available_parallelism()?.get();
        let mut handles: Vec<JoinHandle<std::io::Result<()>>> = Vec::with_capacity(shards_total);

        let mut senders = Vec::with_capacity(shards_total);
        let mut receivers = Vec::with_capacity(shards_total);
        for _ in 0..shards_total {
            let (tx, rx) = crossfire::mpsc::bounded_blocking_async(16);
            senders.push(tx);
            receivers.push(Some(rx));
        }

        let (stop_tx, stop_rx) = crossfire::mpmc::Null::new().new_async();

        for id in 0..shards_total {
            let shard_builder = self.shard_builder.clone();
            let senders = senders.clone();
            let receiver = receivers[id].take().unwrap();
            let stop_rx = stop_rx.clone();

            let handle = std::thread::Builder::new()
                .name(format!("shard-{id}"))
                .spawn(move || {
                    shard_builder.run(id as u16, senders, receiver, stop_rx)?;
                    Ok(())
                })
                .unwrap_or_else(|e| panic!("failed to spawn thread for shard-{id}: {e}"));
            handles.push(handle);
        }

        drop(senders);
        drop(receivers);

        compio::runtime::spawn(async move {
            _ = compio::signal::ctrl_c().await;
            drop(stop_tx);
        })
        .detach();

        for (id, handle) in handles.into_iter().enumerate() {
            if let Err(e) = handle.join() {
                warn!("panicked to join shard-{id} thread: {e:?}");
            }
        }

        info!("node completed");

        Ok(())
    }
}
