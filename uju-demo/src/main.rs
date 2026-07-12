use tracing_subscriber;

fn main() {
    tracing_subscriber::fmt::init();

    println!("Hello, world!");
}
