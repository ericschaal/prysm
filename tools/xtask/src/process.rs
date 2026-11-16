//! Running child processes from the repository root.
use anyhow::{Context, Result, ensure};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The repository root; xtask lives in `tools/xtask`.
pub fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("tools/xtask is two levels below the repository root")
}

/// Cargo's target directory: `CARGO_TARGET_DIR` or `target/`.
pub fn target_dir() -> PathBuf {
    root().join(std::env::var_os("CARGO_TARGET_DIR").unwrap_or_else(|| "target".into()))
}

/// A command for `program` that runs in the repository root.
pub fn command(program: &str, args: &[&str]) -> Command {
    let mut command = Command::new(program);
    command.args(args).current_dir(root());
    command
}

/// Echoes and runs `command`, failing unless it exits successfully.
pub fn run(command: &mut Command) -> Result<()> {
    let line = display(command);
    eprintln!("$ {line}");
    let status = command
        .status()
        .with_context(|| format!("could not start `{line}`"))?;
    ensure!(status.success(), "`{line}` failed ({status})");
    Ok(())
}

/// Runs `command` quietly and returns its trimmed standard output.
pub fn output(command: &mut Command) -> Result<String> {
    let line = display(command);
    let output = command
        .output()
        .with_context(|| format!("could not start `{line}`"))?;
    ensure!(
        output.status.success(),
        "`{line}` failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let stdout =
        String::from_utf8(output.stdout).with_context(|| format!("`{line}` printed non-UTF-8"))?;
    Ok(stdout.trim().to_owned())
}

/// The command as a shell-like line, for messages.
pub fn display(command: &Command) -> String {
    std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}
