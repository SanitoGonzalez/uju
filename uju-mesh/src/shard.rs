use std::rc::Rc;
use std::time::{Duration, Instant};

use futures_util::{FutureExt, select};
use tracing::{error, info};

pub type Sender = crossfire::MTx<crossfire::mpsc::Array<Message>>;
pub type Receiver = crossfire::AsyncRx<crossfire::mpsc::Array<Message>>;
pub type StopReceiver = crossfire::MAsyncRx<crossfire::mpmc::Null>;

pub struct Shard {
    id: u16,
    senders: Vec<Sender>,
    receiver: Option<Receiver>,
}

impl Shard {
    async fn run(self: Rc<Self>, tick_interval: Duration, stop_rx: StopReceiver) {
        let shard = self.clone();
        let stop_rx_ = stop_rx.clone();
        compio::runtime::spawn(async move {
            let mut ticked_at = Instant::now();
            let mut interval = compio::time::interval(tick_interval);

            loop {
                select! {
                    _ = interval.tick().fuse() => {
                        let now = Instant::now();
                        shard.tick((ticked_at - now).as_secs_f32());
                        ticked_at = now;
                    },
                    _ = stop_rx_.recv().fuse() => {
                        break;
                    }
                }
            }
        })
        .detach();

        _ = stop_rx.recv().await;

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
        senders: Vec<Sender>,
        receiver: Receiver,
        stop_rx: StopReceiver,
    ) -> std::io::Result<()> {
        let Self { tick_interval } = self;

        let mut proactor = compio::driver::ProactorBuilder::new();
        proactor
            .capacity(4096) // todo: accept env variable
            .single_issuer(true)
            .defer_taskrun(true)
            .thread_pool_limit(0);

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
            shard.run(tick_interval, stop_rx).await;
        });

        Ok(())
    }
}
