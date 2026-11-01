//! Read-only native Luna integration proof; capture remains in the existing service.
#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(clippy::undocumented_unsafe_blocks)]
#[allow(unsafe_code)]
mod luna;
mod status;

use anyhow::{Result, ensure};
use clap::Parser;
use signal_hook::consts::{SIGINT, SIGTERM};
use std::sync::{Arc, atomic::AtomicBool};
use std::time::Duration;

#[derive(Parser)]
struct Cli {
    /// Separate identity until native activation and permissions have been verified.
    #[arg(long, default_value = "io.prysm.capture.probe.service")]
    service_name: String,
    /// The proof exits automatically; it never changes boot configuration.
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..=300))]
    run_for: u64,
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
    luna::run(&cli.service_name, Duration::from_secs(cli.run_for), stop)
}
