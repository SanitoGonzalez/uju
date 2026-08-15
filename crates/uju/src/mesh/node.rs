use std::thread::JoinHandle;

use tracing::{info, warn};

use crate::mesh::shard;

pub type Id = u16;

static mut ID: Id = u16::MAX;

/// Get the current node ID
pub fn current() -> Id {
    unsafe { ID }
}

#[derive(Debug, Clone)]
pub struct Builder {
    id: Id,
    shard_builder: shard::Builder,
}

impl Builder {
    pub fn new(id: Id) -> Self {
        Self {
            id,
            shard_builder: shard::Builder::new(),
        }
    }

    pub fn with_shard(mut self, builder: shard::Builder) -> Self {
        self.shard_builder = builder;
        self
    }

    pub fn run(self) -> std::io::Result<()> {
        unsafe { ID = self.id }

        let runtime = compio::runtime::Runtime::new()?;
        runtime.block_on(async move {
            self.run_inner().await?;
            Ok(())
        })
    }

    async fn run_inner(self) -> std::io::Result<()> {
        use nix::{
            sched::{CpuSet, sched_getaffinity, sched_setaffinity},
            unistd::Pid,
        };

        let mut allowed_cpus: Vec<usize> = {
            let mask = sched_getaffinity(Pid::from_raw(0))?;
            (0..CpuSet::count())
                .filter(|&cpu| mask.is_set(cpu).unwrap_or(false))
                .collect()
        };

        let shards_total = allowed_cpus.len();
        let mut handles: Vec<JoinHandle<std::io::Result<()>>> = Vec::with_capacity(shards_total);

        let mut senders = Vec::with_capacity(shards_total);
        let mut receivers = Vec::with_capacity(shards_total);
        for _ in 0..shards_total {
            let (tx, rx) = crossfire::mpsc::bounded_blocking_async(16);
            senders.push(tx);
            receivers.push(Some(rx));
        }

        let (shutdown, token) = crossfire::mpmc::Null::new().new_async();

        for (id, cpu) in allowed_cpus.drain(..).enumerate() {
            let shard_builder = self.shard_builder.clone();
            let senders = senders.clone();
            let receiver = receivers[id].take().unwrap();

            // todo: support Count, Range, NUMA
            let mut cpuset = CpuSet::new();
            cpuset.set(cpu)?;

            let token = token.clone();
            let handle = std::thread::Builder::new()
                .name(format!("shard-{id}"))
                .spawn(move || {
                    sched_setaffinity(Pid::from_raw(0), &cpuset)?;
                    info!("[shard-{id}] thread pinned on cpu {cpu}");

                    shard_builder.run(id as shard::Id, senders, receiver, token)?;
                    Ok(())
                })
                .unwrap_or_else(|e| panic!("failed to spawn thread for shard-{id}: {e}"));
            handles.push(handle);
        }
        drop(allowed_cpus);

        drop(senders);
        drop(receivers);

        compio::runtime::spawn(async move {
            _ = compio::signal::ctrl_c().await;

            drop(shutdown);
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
