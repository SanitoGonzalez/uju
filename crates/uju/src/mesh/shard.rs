use std::cell::{Cell, RefCell};
use std::net::SocketAddr;
use std::num::NonZero;
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures_util::{FutureExt, select};
use tracing::{error, info};

use crate::ecs::world::World;
use crate::mesh::error::Result;
use crate::net::transport::tcp;
use crate::util::stop::{StopSource, StopToken};

pub type Id = u16;

thread_local! { static ID: Cell<Id> = const { Cell::new(Id::MAX) } }

/// Get the current shard ID
#[inline(always)]
pub fn current() -> Id {
    ID.get()
}

pub type FrameTx = crossfire::MTx<crossfire::mpsc::Array<Message>>;
pub type FrameRx = crossfire::AsyncRx<crossfire::mpsc::Array<Message>>;
pub type ShutdownTx = crossfire::null::CloseHandle<crossfire::mpmc::Null>;
pub type ShutdownRx = crossfire::MAsyncRx<crossfire::mpmc::Null>;


pub struct Shard {
    id: Id,
    senders: Vec<FrameTx>,
    receiver: Option<FrameRx>,
    stop: StopSource,
    token: StopToken,

    // todo: replace to `UnsafeCell` after stabilization for optimization
    world: RefCell<World>,
}

impl Shard {
    async fn run(self: Rc<Self>, tick_interval: Duration, shutdown: ShutdownRx) -> Result<()> {
        ID.set(self.id);

        self.spawn_tcp_server()?;
        self.spawn_tick(tick_interval);

        _ = shutdown.recv().await;
        self.stop.request();

        // todo: cleanup
        info!("[shard-{}] stopping", self.id);
        Ok(())
    }

    fn spawn_tick(self: &Rc<Self>, tick_interval: Duration) {
        let shard = self.clone();
        let token = self.token.clone();

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
                    _ = token.wait().fuse() => {
                        break;
                    }
                }
            }
        })
        .detach();
    }

    fn spawn_tcp_server(self: &Rc<Self>) -> Result<()> {
        let addr: SocketAddr = "127.0.0.1:7000".parse().unwrap(); // todo: accept config
        let listener = tcp::listener::bind(addr)?;
        let token = self.token.clone();

        compio::runtime::spawn(async move {
            tcp::listener::accept(listener, token).await;
        })
        .detach();
        Ok(())
    }

    fn tick(self: &Rc<Self>, _dt: f32) {
        info!("[shard-{}] ticking", self.id);
    }
}

pub struct Message {}

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
        senders: Vec<FrameTx>,
        receiver: FrameRx,
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
            senders,
            receiver: Some(receiver),
            stop,
            token,
            world: RefCell::new(World::new()),
        });

        runtime.block_on(async move {
            if let Err(e) = shard.run(tick_interval, shutdown).await {
                error!("[shard-{}] stopped with an error: {:?}", id, e);
            }
        });

        Ok(())
    }
}
