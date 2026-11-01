//! Native Luna supervisor, with an explicitly bounded isolated probe mode.
#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(clippy::undocumented_unsafe_blocks)]
mod application;
mod config;
mod files;
#[allow(unsafe_code)]
mod luna;
mod setup;
mod status;
mod supervisor;
mod worker;

use anyhow::{Context, Result, ensure};
use clap::Parser;
use signal_hook::consts::{SIGINT, SIGTERM};
use std::sync::{Arc, atomic::AtomicBool};
use std::time::Duration;

#[derive(Parser)]
struct Cli {
    /// Register the packaged native service and capture permissions, then activate it.
    #[arg(long, conflicts_with = "run_for")]
    setup: bool,
    /// Override the separate identity in bounded probe/sandbox mode.
    #[arg(long, requires = "run_for")]
    service_name: Option<String>,
    /// Use a separate read-only probe, or bound --sandbox, to 1..300 seconds.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=300))]
    run_for: Option<u64>,
    /// Enable the supervisor only in a private temporary test directory.
    #[arg(long)]
    sandbox: Option<std::path::PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    ensure!(
        cfg!(target_os = "linux"),
        "the Luna service requires webOS Linux"
    );
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [SIGINT, SIGTERM] {
        signal_hook::flag::register(signal, Arc::clone(&stop))?;
    }
    let testing = cli.run_for.is_some() || cli.sandbox.is_some();
    let files = if let Some(root) = cli.sandbox.as_deref() {
        Some(files::Files::sandbox(root)?)
    } else if testing {
        None
    } else {
        ensure!(
            nix::unistd::Uid::effective().is_root(),
            "TV service requires root"
        );
        Some(files::Files::production()?)
    };
    let name = cli.service_name.as_deref().unwrap_or(if testing {
        "io.prysm.capture.probe.service"
    } else {
        "io.prysm.capture.service"
    });
    if cli.setup {
        return setup::run(
            &files.context("setup requires production paths or --sandbox")?,
            name,
        );
    }
    let app = files.map(application::App::new).transpose()?;
    luna::run(
        name,
        testing.then(|| Duration::from_secs(cli.run_for.unwrap_or(60))),
        stop,
        app,
    )
}
