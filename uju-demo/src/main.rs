use std::time::Duration;

use tracing_subscriber;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let builder = uju::mesh::node::Builder::new()
        .with_shard(uju::mesh::shard::Builder::new().tick_interval(Duration::from_secs(1)));
    builder.run()?;

    Ok(())
}
