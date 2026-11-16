//! `cargo xtask check`: what CI checks, in one ordered run.
use crate::npm::{self, Script};
use crate::process;
use anyhow::{Context, Result};
use std::process::Command;

/// One check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    RustFormat,
    FrontendLint,
    FrontendFormat,
    Contracts,
    FrontendTests,
    Clippy,
    RustTests,
    Doctests,
    ScriptTests,
    PackageTests,
    DocLinks,
}

impl Step {
    /// The order matters twice: `codegen:check` runs before `npm test`, whose
    /// build rewrites the contracts, and `npm test` writes `frontend/dist`,
    /// which the hosts `include!`, before anything compiles them.
    const ALL: [Self; 11] = [
        Self::RustFormat,
        Self::FrontendLint,
        Self::FrontendFormat,
        Self::Contracts,
        Self::FrontendTests,
        Self::Clippy,
        Self::RustTests,
        Self::Doctests,
        Self::ScriptTests,
        Self::PackageTests,
        Self::DocLinks,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::RustFormat => "Rust formatting",
            Self::FrontendLint => "frontend lint",
            Self::FrontendFormat => "frontend formatting",
            Self::Contracts => "generated contracts",
            Self::FrontendTests => "frontend build and tests",
            Self::Clippy => "clippy",
            Self::RustTests => "Rust tests",
            Self::Doctests => "doctests",
            Self::ScriptTests => "script tests",
            Self::PackageTests => "TV package tests",
            Self::DocLinks => "documentation links",
        }
    }

    fn command(self) -> Command {
        let cargo = |args: &[&str]| process::command("cargo", args);
        let python = |args: &[&str]| {
            let mut command = process::command("python3", args);
            command.env("PYTHONDONTWRITEBYTECODE", "1");
            command
        };
        match self {
            Self::RustFormat => cargo(&["fmt", "--all", "--check"]),
            Self::FrontendLint => Script::Lint.command(),
            Self::FrontendFormat => Script::FormatCheck.command(),
            Self::Contracts => Script::CodegenCheck.command(),
            Self::FrontendTests => Script::Test.command(),
            Self::Clippy => cargo(&[
                "clippy",
                "--locked",
                "--workspace",
                "--all-targets",
                "--all-features",
                "--",
                "-D",
                "warnings",
            ]),
            Self::RustTests => cargo(&[
                "test",
                "--locked",
                "--workspace",
                "--all-targets",
                "--all-features",
            ]),
            // --all-targets leaves out doctests.
            Self::Doctests => {
                cargo(&["test", "--locked", "--workspace", "--doc", "--all-features"])
            }
            Self::ScriptTests => python(&["-m", "unittest", "discover", "-s", "scripts/tests"]),
            Self::PackageTests => {
                python(&["-m", "unittest", "discover", "-s", "packaging/webos/tests"])
            }
            Self::DocLinks => python(&["scripts/check-doc-links.py"]),
        }
    }
}

/// Runs every step in order and stops at the first failure.
pub fn run() -> Result<()> {
    npm::install_if_missing()?;
    for step in Step::ALL {
        eprintln!("==> {}", step.name());
        process::run(&mut step.command())
            .with_context(|| format!("check failed: {}", step.name()))?;
    }
    eprintln!("All checks passed.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(step: Step) -> usize {
        Step::ALL.iter().position(|&s| s == step).unwrap()
    }

    #[test]
    fn contracts_are_checked_before_the_build_rewrites_them() {
        assert!(position(Step::Contracts) < position(Step::FrontendTests));
    }

    #[test]
    fn the_bundle_exists_before_rust_compiles_the_hosts() {
        for step in [Step::Clippy, Step::RustTests, Step::Doctests] {
            assert!(position(Step::FrontendTests) < position(step), "{step:?}");
        }
    }

    #[test]
    fn clippy_denies_warnings_on_every_target() {
        let line = process::display(&Step::Clippy.command());
        assert!(
            line.ends_with("--all-targets --all-features -- -D warnings"),
            "{line}"
        );
    }
}
