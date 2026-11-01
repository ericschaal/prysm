use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Prysm capture, processing, rendering, and REST API")]
struct Args {
    #[arg(long, global = true, default_value = "prysm.toml")]
    config: PathBuf,
    /// Opt-in Prometheus endpoint, separate from the REST API
    #[arg(long, global = true, value_name = "IP:PORT")]
    metrics_listen: Option<std::net::SocketAddr>,
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
        None => {
            let _metrics = args.metrics_listen.and_then(|address| {
                match prysm_metrics::start(address, prysm_metrics::Role::Prysm) {
                    Ok(exporter) => {
                        tracing::info!(address=%exporter.address(), "metrics listening");
                        Some(exporter)
                    }
                    Err(error) => {
                        tracing::warn!(error=%format!("{error:#}"), "metrics disabled");
                        None
                    }
                }
            });
            prysm::run(&args.config, config)
        }
    }
}
