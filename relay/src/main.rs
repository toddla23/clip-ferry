use clap::Parser;

/// clip-ferry 릴레이 서버
#[derive(Parser, Debug)]
#[command(name = "clip-ferry-relay", version, about)]
struct Args {}

fn main() -> anyhow::Result<()> {
    let _args = Args::parse();
    Ok(())
}
