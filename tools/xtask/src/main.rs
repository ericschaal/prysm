//! `cargo xtask`: one entry point for generating, checking and building
//! Prysm. It runs the existing tools (Cargo, the npm scripts in `frontend/`,
//! `packaging/docker/Dockerfile`, `packaging/webos/build-ipk.sh` and the
//! Python checks) in the right order; it does not replace them. Every command
//! runs from the repository root. `release-artifacts` checks a release's CI
//! artifacts for `packaging/release/publish.sh`.
mod check;
mod codegen;
mod dist;
mod doctor;
mod manifest;
mod npm;
mod process;
mod release;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
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
    Check {
        /// Only these groups (one per CI job), in the usual order; all when none.
        #[arg(value_enum)]
        groups: Vec<check::Group>,
        /// Run every step and report all failures at the end.
        #[arg(long)]
        keep_going: bool,
    },
    /// Build artifacts into dist/ and record them in dist/manifest.json.
    Dist {
        #[arg(required = true)]
        targets: Vec<dist::Target>,
    },
    /// Report which tools are installed and which commands need them.
    Doctor,
    /// Check a release's CI artifacts against their manifests, then move the
    /// files up into DIR (packaging/release/publish.sh runs this).
    ReleaseArtifacts {
        /// One directory per CI artifact, each with its manifest.json.
        dir: PathBuf,
        /// The version every file must have.
        #[arg(long)]
        version: String,
        /// The commit every file must be built from.
        #[arg(long)]
        commit: String,
    },
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
        Command::Check { groups, keep_going } => check::run(&groups, keep_going)?,
        Command::Dist { targets } => dist::run(&targets)?,
        Command::Doctor => return Ok(doctor::run()),
        Command::ReleaseArtifacts {
            dir,
            version,
            commit,
        } => release::run(&dir, &version, &commit)?,
    }
    Ok(ExitCode::SUCCESS)
}
