//! Native webOS capture sender. Hardware resources never cross threads.
#![deny(unsafe_code)]
mod capture;
mod cli;
#[allow(unsafe_code)]
mod native;
mod pixels;
mod session;
mod transport;

use anyhow::{Result, ensure};
use clap::Parser;
use cli::Cli;
use signal_hook::consts::{SIGINT, SIGTERM};
use std::sync::{Arc, atomic::AtomicBool};

fn main() -> Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    ensure!(cfg!(target_os = "linux"), "capture requires webOS Linux");
    ensure!(
        nix::unistd::Uid::effective().is_root(),
        "TV capture requires root"
    );
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [SIGINT, SIGTERM] {
        signal_hook::flag::register(signal, Arc::clone(&stop))?;
    }
    let _metrics = cli.metrics_listen.and_then(|address| {
        match prysm_metrics::start(address, prysm_metrics::Role::Capturer) {
            Ok(exporter) => {
                tracing::info!(address = %exporter.address(), "metrics listening");
                Some(exporter)
            }
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), "metrics disabled");
                None
            }
        }
    });
    session::run(&cli, &stop)
}
