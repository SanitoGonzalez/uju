use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {}

fn main() {
    let args = Args::parse();
}
