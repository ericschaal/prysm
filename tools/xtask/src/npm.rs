//! The npm scripts of the `frontend/` workspace.
use crate::process;
use anyhow::Result;
use std::process::Command;

/// A script from `frontend/package.json`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    /// Generate the contracts from Rust (it runs the exporter itself).
    Codegen,
    /// Fail if the generated contracts differ, without writing.
    CodegenCheck,
    /// Codegen, typecheck and the bundles in `frontend/dist`.
    Build,
    Lint,
    FormatCheck,
    /// `build`, then the contract and DOM tests.
    Test,
}

impl Script {
    fn name(self) -> &'static str {
        match self {
            Self::Codegen => "codegen",
            Self::CodegenCheck => "codegen:check",
            Self::Build => "build",
            Self::Lint => "lint",
            Self::FormatCheck => "format:check",
            Self::Test => "test",
        }
    }

    /// `npm --prefix frontend run SCRIPT`.
    pub fn command(self) -> Command {
        process::command("npm", &["--prefix", "frontend", "run", self.name()])
    }
}

/// Installs the locked packages (`npm ci`) when `frontend/node_modules` is
/// missing, as on a fresh checkout. An existing install is left alone.
pub fn install_if_missing() -> Result<()> {
    if process::root().join("frontend/node_modules").is_dir() {
        return Ok(());
    }
    process::run(&mut process::command(
        "npm",
        &["--prefix", "frontend", "ci", "--no-audit", "--no-fund"],
    ))
}

/// Runs `script`, installing the packages first if needed.
pub fn run(script: Script) -> Result<()> {
    install_if_missing()?;
    process::run(&mut script.command())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_run_in_the_frontend_workspace() {
        let command = Script::CodegenCheck.command();
        assert_eq!(
            process::display(&command),
            "npm --prefix frontend run codegen:check"
        );
    }
}
