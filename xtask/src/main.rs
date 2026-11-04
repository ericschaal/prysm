//! Repository workflows; Cargo and Vite still own compilation.
use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const HOST: &str = "crates/web/prysm-web/control-ui";
const WEBOS: &str = "crates/web/prysm-webos";

#[derive(Parser)]
#[command(about = "Build, verify, generate contracts, and package Prysm")]
struct Cli {
    #[command(subcommand)]
    task: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Install locked frontend dependencies (both UIs by default).
    Setup {
        #[arg(value_enum)]
        ui: Option<Ui>,
    },
    /// Verify Rust, frontends, and packaging helpers (all by default).
    Check {
        #[arg(value_enum)]
        suite: Option<Suite>,
    },
    /// Generate committed JSON schemas and TypeScript (both contracts by default).
    Codegen {
        #[arg(value_enum)]
        contract: Option<Contract>,
        /// Fail on stale or missing output without writing files.
        #[arg(long)]
        check: bool,
    },
    /// Build and validate a package; does not install or deploy it.
    Package {
        #[arg(value_enum)]
        target: Package,
        /// Include the experimental TV hardware encoder.
        #[arg(long)]
        hardware_h264: bool,
        /// Allow the webOS app to run Prysm on the TV.
        #[arg(long)]
        embedded_prysm: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Ui {
    Host,
    Webos,
}
#[derive(Clone, Copy, ValueEnum)]
enum Suite {
    Rust,
    Frontend,
}
#[derive(Clone, Copy, ValueEnum)]
enum Contract {
    Api,
    Webos,
}
#[derive(Clone, Copy, ValueEnum)]
enum Package {
    Deb,
    Docker,
    Webos,
}

fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

fn run(command: &mut Command) -> Result<()> {
    eprintln!("+ {command:?}");
    ensure!(
        command
            .status()
            .with_context(|| format!("run {command:?}"))?
            .success(),
        "command failed: {command:?}"
    );
    Ok(())
}

fn capture(command: &mut Command) -> Result<String> {
    let output = command
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("run {command:?}"))?;
    ensure!(output.status.success(), "command failed: {command:?}");
    String::from_utf8(output.stdout).context("command output is not UTF-8")
}

fn npm(ui: &str, args: &[&str]) -> Result<()> {
    run(Command::new("npm").arg("--prefix").arg(ui).args(args))
}

fn generate(generator: &[&str], schema: &str) -> Result<String> {
    let mut child = Command::new("node")
        .current_dir(HOST)
        .args(generator)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("run contract generator; run cargo xtask setup first")?;
    let write = child
        .stdin
        .take()
        .context("generator stdin")?
        .write_all(schema.as_bytes());
    let output = child.wait_with_output()?;
    ensure!(
        output.status.success(),
        "contract generation failed; run cargo xtask setup first"
    );
    write?;
    String::from_utf8(output.stdout).context("generated contract is not UTF-8")
}

fn emit(outputs: &[(PathBuf, String)], check: bool) -> Result<()> {
    // Validate the entire set before writing any generated file.
    if check {
        for (path, generated) in outputs {
            ensure!(
                fs::read_to_string(path).ok().as_ref() == Some(generated),
                "{} is stale or missing; run cargo xtask codegen",
                path.display()
            );
        }
    } else {
        for (path, generated) in outputs {
            if fs::read_to_string(path).ok().as_ref() != Some(generated) {
                fs::write(path, generated).with_context(|| format!("write {}", path.display()))?;
            }
        }
    }
    Ok(())
}

fn codegen(contract: Option<Contract>, check: bool) -> Result<()> {
    let contracts =
        contract.map_or_else(|| vec![Contract::Api, Contract::Webos], |value| vec![value]);
    let mut outputs = Vec::new();
    for contract in contracts {
        let mut command = cargo();
        command.args(["run", "--locked", "--quiet", "-p"]);
        let (schema_path, types_path) = match contract {
            Contract::Api => {
                command.args(["prysm-api", "--bin", "export-openapi"]);
                (
                    format!("{HOST}/openapi.json"),
                    format!("{HOST}/api.generated.ts"),
                )
            }
            Contract::Webos => {
                command.args([
                    "prysm-webos-service",
                    "--features",
                    "schema",
                    "--bin",
                    "export-service-schema",
                ]);
                (
                    format!("{WEBOS}/service.schema.json"),
                    format!("{WEBOS}/src/service.generated.d.ts"),
                )
            }
        };
        let schema = capture(&mut command)?;
        let types = match contract {
            Contract::Api => generate(&["generate-types.mjs"], &schema)?,
            Contract::Webos => {
                let script = "generate-service-validators.mjs";
                let validators = generate(&[script], &schema)?;
                outputs.push((
                    format!("{WEBOS}/src/service.generated.js").into(),
                    validators,
                ));
                generate(&[script, "types"], &schema)?
            }
        };
        outputs.push((schema_path.into(), schema));
        outputs.push((types_path.into(), types));
    }
    emit(&outputs, check)
}

fn check(suite: Option<Suite>) -> Result<()> {
    if suite.is_none() || matches!(suite, Some(Suite::Frontend)) {
        codegen(None, true)?;
        npm(".", &["run", "lint"])?;
        npm(".", &["run", "format:check"])?;
        npm(HOST, &["test"])?;
        npm(WEBOS, &["test"])?;
        run(Command::new("python3")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .args(["-m", "unittest", "discover", "-s", "packaging/webos/tests"]))?;
    }
    if suite.is_none() {
        for directory in ["packaging/debian/tests", "scripts/tests"] {
            run(Command::new("python3")
                .env("PYTHONDONTWRITEBYTECODE", "1")
                .args(["-m", "unittest", "discover", "-s", directory]))?;
        }
        run(Command::new("python3").arg("scripts/check-doc-links.py"))?;
    }
    if suite.is_none() || matches!(suite, Some(Suite::Rust)) {
        run(cargo().args(["fmt", "--all", "--", "--check"]))?;
        run(cargo().args(["test", "--locked", "--workspace"]))?;
        run(cargo().args([
            "clippy",
            "--locked",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ]))?;
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    std::env::set_current_dir(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .context("workspace root")?,
    )?;
    match cli.task {
        Task::Setup { ui } => {
            npm(".", &["ci", "--no-audit", "--no-fund"])?;
            if ui.is_none() || matches!(ui, Some(Ui::Host)) {
                npm(HOST, &["ci", "--no-audit", "--no-fund"])?;
            }
            if ui.is_none() || matches!(ui, Some(Ui::Webos)) {
                npm(WEBOS, &["ci", "--no-audit", "--no-fund"])?;
            }
            Ok(())
        }
        Task::Check { suite } => check(suite),
        Task::Codegen { contract, check } => codegen(contract, check),
        Task::Package {
            target,
            hardware_h264,
            embedded_prysm,
        } => {
            ensure!(
                !(hardware_h264 || embedded_prysm) || matches!(target, Package::Webos),
                "TV feature flags are only supported for webos packages"
            );
            match target {
                Package::Deb => {
                    codegen(Some(Contract::Api), true)?;
                    run(cargo().args([
                        "build",
                        "--locked",
                        "--release",
                        "-p",
                        "prysm",
                        "--no-default-features",
                        "--bins",
                    ]))?;
                    run(cargo().args(["deb", "-p", "prysm", "--no-build"]))
                }
                Package::Docker => {
                    run(Command::new("docker").args(["build", "--tag", "prysm:local", "."]))
                }
                Package::Webos => {
                    codegen(Some(Contract::Webos), true)?;
                    let mut command = Command::new("bash");
                    command.arg("packaging/webos/build-ipk.sh");
                    if hardware_h264 {
                        command.arg("--hardware-h264");
                    }
                    if embedded_prysm {
                        command.arg("--embedded-prysm");
                    }
                    run(&mut command)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_never_repairs_stale_or_missing_files() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("schema.json");
        let second = directory.path().join("types.ts");
        fs::write(&first, "old").unwrap();
        let output = vec![
            (first.clone(), "new".into()),
            (second.clone(), "types".into()),
        ];
        assert!(emit(&output, true).is_err());
        assert_eq!(fs::read_to_string(&first).unwrap(), "old");
        assert!(!second.exists());
        emit(&output, false).unwrap();
        emit(&output, true).unwrap();
        fs::remove_file(&second).unwrap();
        assert!(emit(&output, true).is_err());
        assert!(!second.exists());
    }
}
