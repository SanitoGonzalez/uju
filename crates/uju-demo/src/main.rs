use std::time::Duration;

use tracing_subscriber;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let builder = uju::cluster::node::Builder::new(0)
        .with_shard(uju::cluster::shard::Builder::new().tick_interval(Duration::from_secs(1)));
    builder.run()?;

    Ok(())
}
