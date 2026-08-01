/// Unique shard ID
pub type Id = u16;

pub type Sender = crossfire::MTx<crossfire::mpsc::Array<Message>>;
pub type Receiver = crossfire::AsyncRx<crossfire::mpsc::Array<Message>>;

pub struct Shard {
    id: Id,
    runtime: compio::runtime::Runtime,
    senders: Vec<Sender>,
    receiver: Receiver,
}

impl Shard {
    pub fn run(&self) {
        self.runtime.block_on(async {})
    }
}

pub struct Message {}

#[derive(Debug, Clone)]
pub struct Builder {}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Builder {
    pub fn new() -> Self {
        Self {}
    }

    pub fn build(
        &self,
        id: Id,
        senders: Vec<Sender>,
        receiver: Receiver,
    ) -> std::io::Result<Shard> {
        let mut proactor = compio::driver::ProactorBuilder::new();
        proactor
            .single_issuer(true)
            .defer_taskrun(true)
            .thread_pool_limit(0);

        let runtime = compio::runtime::RuntimeBuilder::new()
            .with_proactor(proactor)
            .build()?;

        Ok(Shard {
            id,
            runtime,
            senders,
            receiver,
        })
    }
}
