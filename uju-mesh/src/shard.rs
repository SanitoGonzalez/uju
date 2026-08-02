use std::num::NonZero;
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures_util::{FutureExt, select};
use tracing::{error, info};

pub type FrameTx = crossfire::MTx<crossfire::mpsc::Array<Message>>;
pub type FrameRx = crossfire::AsyncRx<crossfire::mpsc::Array<Message>>;
pub type ShutdownTx = crossfire::null::CloseHandle<crossfire::mpmc::Null>;
pub type ShutdownRx = crossfire::MAsyncRx<crossfire::mpmc::Null>;

pub struct Shard {
    id: u16,
    senders: Vec<FrameTx>,
    receiver: Option<FrameRx>,
}

impl Shard {
    async fn run(self: Rc<Self>, tick_interval: Duration, token: ShutdownRx) {
        let shard = self.clone();
        let token_ = token.clone();
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
                    _ = token_.recv().fuse() => {
                        break;
                    }
                }
            }
        })
        .detach();

        _ = token.recv().await;

        // todo: cleanup
        info!("[shard-{}] stopping", self.id);
    }

    fn tick(self: &Rc<Self>, dt: f32) {
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
        id: u16,
        senders: Vec<FrameTx>,
        receiver: FrameRx,
        token: ShutdownRx,
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

        let shard = Rc::new(Shard {
            id,
            senders,
            receiver: Some(receiver),
        });

        runtime.block_on(async move {
            shard.run(tick_interval, token).await;
        });

        Ok(())
    }
}
