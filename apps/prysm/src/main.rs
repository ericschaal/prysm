use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use prysm::{SettingsStore, WebConfig};
use std::path::PathBuf;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
#[command(about = "Prysm capture, processing, rendering, and REST API")]
struct Args {
    #[arg(
        long,
        global = true,
        env = "PRYSM_CONFIG",
        default_value = "settings.json"
    )]
    config: PathBuf,
    /// HTTP listener address.
    #[arg(
        long,
        global = true,
        env = "PRYSM_LISTEN",
        default_value = "0.0.0.0:8787"
    )]
    listen: std::net::SocketAddr,
    /// Allowed web authorities, including the externally published port.
    #[arg(
        long = "allowed-host",
        global = true,
        env = "PRYSM_ALLOWED_HOSTS",
        value_delimiter = ',',
        default_value = "localhost:8787,127.0.0.1:8787"
    )]
    allowed_hosts: Vec<String>,
    /// Opt-in Prometheus endpoint, separate from the REST API
    #[arg(
        long,
        global = true,
        env = "PRYSM_METRICS_LISTEN",
        value_name = "IP:PORT"
    )]
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
    let filter = std::env::var("RUST_LOG")
        .unwrap_or_else(|_| "info".into())
        .parse::<tracing_subscriber::filter::Targets>()
        .context("invalid RUST_LOG target filter")?;
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();
    let web = WebConfig {
        listen: args.listen,
        allowed_hosts: args.allowed_hosts,
    };
    web.validate()?;
    let settings = SettingsStore::load(args.config)?;
    match args.command {
        Some(Command::Check) => {
            anyhow::ensure!(
                settings.config().is_some(),
                "settings are incomplete; open the web panel to finish setup"
            );
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
            prysm::run(settings, web)
        }
    }
}
