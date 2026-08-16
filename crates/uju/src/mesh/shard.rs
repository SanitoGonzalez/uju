pub mod message;

use std::cell::{Cell, RefCell};
use std::net::SocketAddr;
use std::num::NonZero;
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures_util::{FutureExt, select};
use tracing::{error, info};

use crate::ecs::world::World;
use crate::mesh::error::Result;
use crate::mesh::shard::message::{Frame, Message, ReplicationOp};
use crate::net::transport::tcp;
use crate::util::stop::{StopSource, StopToken};

pub type Id = u16;

thread_local! { static ID: Cell<Id> = const { Cell::new(Id::MAX) } }

/// Get the current shard ID
#[inline(always)]
pub fn current() -> Id {
    ID.get()
}

pub type FrameTx = crossfire::MTx<crossfire::mpsc::Array<Frame>>;
pub type FrameRx = crossfire::AsyncRx<crossfire::mpsc::Array<Frame>>;
pub type ShutdownTx = crossfire::null::CloseHandle<crossfire::mpmc::Null>;
pub type ShutdownRx = crossfire::MAsyncRx<crossfire::mpmc::Null>;

pub struct Shard {
    id: Id,
    tx: Vec<FrameTx>,
    stop: StopSource,
    token: StopToken,
    world: World,
}

impl Shard {
    async fn run(
        self: Rc<Self>,
        rx: FrameRx,
        tick_interval: Duration,
        shutdown: ShutdownRx,
    ) -> Result<()> {
        ID.set(self.id);

        Self::spawn_recv_loop(self.clone(), rx);
        Self::spawn_tcp_server(self.clone())?;
        Self::spawn_tick(self.clone(), tick_interval); // todo: move tick interval into configs

        _ = shutdown.recv().await;
        self.stop.request();

        // todo: cleanup
        info!("[shard-{}] stopping", self.id);
        Ok(())
    }

    fn spawn_recv_loop(shard: Rc<Self>, rx: FrameRx) {
        let token = shard.token.clone();

        compio::runtime::spawn(async move {
            loop {
                select! {
                    result = rx.recv().fuse() => match result {
                        Ok(msg) => shard.process(msg).await,
                        Err(_) => break,
                    },
                    _ = token.wait().fuse() => break,
                }
            }
        })
        .detach();
    }

    fn spawn_tick(shard: Rc<Self>, tick_interval: Duration) {
        let token = shard.token.clone();

        compio::runtime::spawn(async move {
            let mut ticked_at = Instant::now();
            let mut interval = compio::time::interval(tick_interval);

            loop {
                select! {
                    _ = interval.tick().fuse() => {
                        let now = Instant::now();
                        shard.tick((now - ticked_at).as_secs_f32());
                        ticked_at = now;
                    },
                    _ = token.wait().fuse() => break,
                }
            }
        })
        .detach();
    }

    fn spawn_tcp_server(shard: Rc<Self>) -> Result<()> {
        let addr: SocketAddr = "127.0.0.1:7000".parse().unwrap(); // todo: accept config
        let listener = tcp::listener::bind(addr)?;
        let token = shard.token.clone();

        compio::runtime::spawn(async move {
            tcp::listener::accept(listener, token).await;
        })
        .detach();
        Ok(())
    }

    fn tick(self: &Rc<Self>, _dt: f32) {
        info!("[shard-{}] ticking", self.id);
    }

    async fn process(self: &Rc<Self>, frame: Frame) {
        let Frame {
            node,
            shard,
            msg,
            result_tx,
        } = frame;

        // todo: extract/refactor
        match msg {
            Message::Replication(ops) => {
                for op in ops {
                    match op {
                        ReplicationOp::Upsert {
                            universal,
                            components,
                        } => {
                            let local = self
                                .world
                                .replicas
                                .borrow()
                                .local(universal)
                                .unwrap_or_else(|| {
                                    // let replica = world.spawn_replica();
                                    // self.world.borrow().replicas.borrow().link(universal, replica);
                                    // replica
                                    todo!()
                                });
                            for (id, component) in components {
                                todo!("apply replication")
                            }
                        }
                        ReplicationOp::Remove { universal } => {
                            // if let Some(replica) = self.world.borrow().replicas.borrow().unlink(universal) {
                            //     self.world.despawn(replica);
                            // }
                            todo!()
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Builder {
    tick_interval: Duration,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Builder {
    pub fn new() -> Self {
        Self {
            tick_interval: Duration::from_millis(100),
        }
    }

    pub fn tick_interval(mut self, tick_interval: Duration) -> Self {
        self.tick_interval = tick_interval;
        self
    }

    pub fn run(
        self,
        id: Id,
        tx: Vec<FrameTx>,
        rx: FrameRx,
        shutdown: ShutdownRx,
    ) -> std::io::Result<()> {
        let Self { tick_interval } = self;

        let mut proactor = compio::driver::ProactorBuilder::new();
        proactor
            .capacity(4096) // todo: accept env variable
            .cqsize(32768) // todo: accept env variable
            .single_issuer(true)
            .defer_taskrun(true)
            .thread_pool_limit(0)
            .buffer_pool_size(NonZero::new(8192).unwrap()) // todo: accept env variable
            .buffer_pool_buffer_len(2048); // todo: accept env variable
        let runtime = compio::runtime::RuntimeBuilder::new()
            .with_proactor(proactor)
            .event_interval(128) // todo: accept env variable
            .build()?;

        let (stop, token) = StopSource::new();
        let shard = Rc::new(Shard {
            id,
            tx,
            stop,
            token,
            world: World::new(),
        });

        runtime.block_on(async move {
            if let Err(e) = shard.run(rx, tick_interval, shutdown).await {
                error!("[shard-{}] stopped with an error: {:?}", id, e);
            }
        });

        Ok(())
    }
}
