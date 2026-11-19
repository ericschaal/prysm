//! `cargo xtask codegen`: files generated from Rust and checked in.
use crate::npm::{self, Script};
use crate::process::{self, target_dir};
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::process::Command;

/// Write the generated files, or only check that they are current.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Write,
    Check,
}

/// The frontend's contracts, then with `luna` the webOS Luna and GLib
/// bindings, which need the SDK in `WEBOS_SDK`.
pub fn run(mode: Mode, luna: bool) -> Result<()> {
    contracts(mode)?;
    if luna {
        process::run(luna_command(mode))?;
    }
    Ok(())
}

/// Runs the Rust exporter once and hands its JSON to the Node generator,
/// which writes TypeScript under `frontend/packages/contracts/generated`.
fn contracts(mode: Mode) -> Result<()> {
    let json = process::output(process::command(
        "cargo",
        &["run", "--quiet", "--locked", "-p", "prysm-export-contracts"],
    ))?;
    let export = target_dir().join("xtask/contracts.json");
    fs::create_dir_all(target_dir().join("xtask"))
        .and_then(|()| fs::write(&export, json))
        .with_context(|| format!("writing {}", export.display()))?;
    npm::install_if_stale()?;
    process::run(generator_command(mode, &export))
}

/// `npm run codegen[:check] -- EXPORT`. The path is absolute: npm runs the
/// script in `frontend/`.
fn generator_command(mode: Mode, export: &Path) -> Command {
    let script = match mode {
        Mode::Write => Script::Codegen,
        Mode::Check => Script::CodegenCheck,
    };
    let mut command = script.command();
    command.arg("--").arg(export);
    command
}

fn luna_command(mode: Mode) -> Command {
    let mut command = process::command(
        "cargo",
        &[
            "run",
            "--quiet",
            "--locked",
            "-p",
            "prysm-webos-luna-sys",
            "--features",
            "codegen",
            "--bin",
            "generate-luna-bindings",
        ],
    );
    if mode == Mode::Check {
        command.args(["--", "--check"]);
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_reads_the_export_by_absolute_path() {
        let command =
            generator_command(Mode::Check, Path::new("/repo/target/xtask/contracts.json"));
        assert_eq!(
            process::display(&command),
            "npm --prefix frontend run codegen:check -- /repo/target/xtask/contracts.json"
        );
    }

    #[test]
    fn luna_bindings_are_checked_without_writing() {
        assert!(process::display(&luna_command(Mode::Check)).ends_with(" -- --check"));
        assert!(!process::display(&luna_command(Mode::Write)).contains("--check"));
    }
}
