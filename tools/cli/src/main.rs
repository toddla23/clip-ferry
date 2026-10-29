use clap::Parser;

/// clip-ferry 개발·검증용 CLI
#[derive(Parser, Debug)]
#[command(name = "clip-ferry-cli", version, about)]
struct Args {}

fn main() -> anyhow::Result<()> {
    let _args = Args::parse();
    Ok(())
}
