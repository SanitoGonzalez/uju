use tracing_subscriber;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let builder = uju::mesh::node::Builder::new();
    builder.run()?;

    Ok(())
}
