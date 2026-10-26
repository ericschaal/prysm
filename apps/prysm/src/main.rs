use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Prysm capture, processing, rendering, and REST API")]
struct Args {
    #[arg(long, global = true, default_value = "prysm.toml")]
    config: PathBuf,
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    /// Validate configuration without starting capture or the REST API.
    Check,
}

fn main() -> Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    let config = prysm::config::load(&args.config)?;
    prysm::validate_config(&config)?;
    match args.command {
        Some(Command::Check) => {
            println!("Configuration is valid");
            Ok(())
        }
        None => prysm::run(&args.config, config),
    }
}
