//! `cargo xtask`: one entry point for generating, checking and building
//! Prysm. It runs the existing tools (Cargo, the npm scripts in `frontend/`,
//! `packaging/docker/Dockerfile`, `packaging/webos/build-ipk.sh` and the
//! Python checks) in the right order; it does not replace them. Every command
//! runs from the repository root.
mod check;
mod codegen;
mod dist;
mod doctor;
mod manifest;
mod npm;
mod process;

use clap::{Parser, Subcommand};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "cargo xtask",
    bin_name = "cargo xtask",
    about = "Build and check Prysm from one place"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Regenerate the frontend's contracts from Rust.
    Codegen {
        /// Fail if the generated files are out of date, without writing them.
        #[arg(long)]
        check: bool,
        /// Also the webOS Luna and GLib bindings; needs the SDK in WEBOS_SDK.
        #[arg(long)]
        luna: bool,
    },
    /// Run the checks to pass before pushing; stops at the first failure.
    Check,
    /// Build artifacts into dist/ and record them in dist/manifest.json.
    Dist {
        #[arg(required = true)]
        targets: Vec<dist::Target>,
    },
    /// Report which tools are installed and which commands need them.
    Doctor,
}

fn main() -> anyhow::Result<ExitCode> {
    match Cli::parse().command {
        Command::Codegen { check, luna } => {
            let mode = if check {
                codegen::Mode::Check
            } else {
                codegen::Mode::Write
            };
            codegen::run(mode, luna)?;
        }
        Command::Check => check::run()?,
        Command::Dist { targets } => dist::run(&targets)?,
        Command::Doctor => return Ok(doctor::run()),
    }
    Ok(ExitCode::SUCCESS)
}
