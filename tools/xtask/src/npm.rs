//! The npm scripts of the `frontend/` workspace. Each does one thing; xtask
//! runs them in order.
use crate::process::{self, root};
use anyhow::Result;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

/// A script from `frontend/package.json`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    /// Write the contracts from the exporter's output (see `codegen`).
    Codegen,
    /// Fail if the contracts differ from the exporter's output, without writing.
    CodegenCheck,
    Typecheck,
    /// The bundles in `frontend/dist`, which the hosts and IPKs take.
    Bundle,
    Lint,
    FormatCheck,
    /// The contract and DOM tests; the TV tests read `frontend/dist`.
    Test,
}

impl Script {
    fn name(self) -> &'static str {
        match self {
            Self::Codegen => "codegen",
            Self::CodegenCheck => "codegen:check",
            Self::Typecheck => "typecheck",
            Self::Bundle => "bundle",
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

/// Installs the locked packages (`npm ci`) unless the install is current.
/// npm writes `node_modules/.package-lock.json` at the end of every install,
/// so an install older than `package-lock.json`, or none, is stale.
pub fn install_if_stale() -> Result<()> {
    let frontend = root().join("frontend");
    let modified = |path: &Path| fs::metadata(path).and_then(|meta| meta.modified()).ok();
    if is_current(
        modified(&frontend.join("node_modules/.package-lock.json")),
        modified(&frontend.join("package-lock.json")),
    ) {
        return Ok(());
    }
    process::run(&mut process::command(
        "npm",
        &["--prefix", "frontend", "ci", "--no-audit", "--no-fund"],
    ))
}

fn is_current(installed: Option<SystemTime>, locked: Option<SystemTime>) -> bool {
    installed.is_some() && installed >= locked
}

/// Runs `script`, installing the packages first if needed.
pub fn run(script: Script) -> Result<()> {
    install_if_stale()?;
    process::run(&mut script.command())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn scripts_run_in_the_frontend_workspace() {
        let command = Script::CodegenCheck.command();
        assert_eq!(
            process::display(&command),
            "npm --prefix frontend run codegen:check"
        );
    }

    #[test]
    fn an_install_older_than_the_lockfile_is_stale() {
        let earlier = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
        let later = earlier + Duration::from_secs(1);
        assert!(is_current(Some(later), Some(earlier)));
        assert!(is_current(Some(later), Some(later)));
        assert!(!is_current(Some(earlier), Some(later)));
        assert!(!is_current(None, Some(later)));
    }
}
