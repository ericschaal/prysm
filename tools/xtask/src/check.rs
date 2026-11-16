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
    Typecheck,
    Bundle,
    FrontendTests,
    Clippy,
    RustTests,
    Doctests,
    ScriptTests,
    PackageTests,
    DocLinks,
}

impl Step {
    /// The order matters twice: stale contracts fail as such before the
    /// typecheck reports their symptoms, and the bundle in `frontend/dist`
    /// exists before the TV tests read it and before Rust compiles the
    /// hosts, which `include!` it.
    const ALL: [Self; 13] = [
        Self::RustFormat,
        Self::FrontendLint,
        Self::FrontendFormat,
        Self::Contracts,
        Self::Typecheck,
        Self::Bundle,
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
            Self::Typecheck => "frontend typecheck",
            Self::Bundle => "frontend bundle",
            Self::FrontendTests => "frontend tests",
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
            // The command a developer runs: it builds and runs the exporter.
            Self::Contracts => cargo(&["xtask", "codegen", "--check"]),
            Self::Typecheck => Script::Typecheck.command(),
            Self::Bundle => Script::Bundle.command(),
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
    npm::install_if_stale()?;
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
    fn contracts_are_checked_before_the_typecheck() {
        assert!(position(Step::Contracts) < position(Step::Typecheck));
    }

    #[test]
    fn the_bundle_exists_before_the_tests_and_rust_read_it() {
        for step in [
            Step::FrontendTests,
            Step::Clippy,
            Step::RustTests,
            Step::Doctests,
        ] {
            assert!(position(Step::Bundle) < position(step), "{step:?}");
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
