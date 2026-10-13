use std::time::Duration;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    uju::init();

    let builder = uju::cluster::node::Builder::new(0)
        .with_shard(uju::cluster::shard::Builder::new().tick_interval(Duration::from_secs(1)));
    builder.run()?;

    Ok(())
}
