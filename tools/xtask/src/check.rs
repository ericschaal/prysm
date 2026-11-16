//! `cargo xtask check`: what CI checks, in one ordered run. CI's `quality`,
//! `frontend` and `rust` jobs each run one [`Group`] of it.
use crate::npm::{self, Script};
use crate::process;
use anyhow::{Context, Result};
use clap::ValueEnum;
use std::process::Command;

/// The checks of one CI job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Group {
    /// Formatting, clippy, tests and doctests of the Cargo workspace.
    Rust,
    /// Lint, formatting, contracts, typecheck, bundle and tests of frontend/.
    Frontend,
    /// The Python checks: script and TV package tests, the acceptance
    /// runner's lint, types, layers and tests, and the documentation links.
    Scripts,
}

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
    RunnerLint,
    RunnerFormat,
    RunnerTypes,
    RunnerLayers,
    RunnerTests,
    DocLinks,
}

impl Step {
    /// The order matters twice: stale contracts fail as such before the
    /// typecheck reports their symptoms, and the bundle in `frontend/dist`
    /// exists before the TV tests read it and before Rust compiles the
    /// hosts, so they embed the real portal rather than the "not built" page.
    const ALL: [Self; 18] = [
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
        Self::RunnerLint,
        Self::RunnerFormat,
        Self::RunnerTypes,
        Self::RunnerLayers,
        Self::RunnerTests,
        Self::DocLinks,
    ];

    fn group(self) -> Group {
        match self {
            Self::RustFormat | Self::Clippy | Self::RustTests | Self::Doctests => Group::Rust,
            Self::FrontendLint
            | Self::FrontendFormat
            | Self::Contracts
            | Self::Typecheck
            | Self::Bundle
            | Self::FrontendTests => Group::Frontend,
            Self::ScriptTests
            | Self::PackageTests
            | Self::RunnerLint
            | Self::RunnerFormat
            | Self::RunnerTypes
            | Self::RunnerLayers
            | Self::RunnerTests
            | Self::DocLinks => Group::Scripts,
        }
    }

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
            Self::RunnerLint => "acceptance runner lint",
            Self::RunnerFormat => "acceptance runner formatting",
            Self::RunnerTypes => "acceptance runner types",
            Self::RunnerLayers => "acceptance runner layers",
            Self::RunnerTests => "acceptance runner tests",
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
        // The runner's own locked environment (docs/developers/acceptance.md).
        let runner = |args: &[&str]| {
            let mut command = process::command(
                "uv",
                &["run", "--locked", "--directory", "scripts/acceptance"],
            );
            command.args(args).env("PYTHONDONTWRITEBYTECODE", "1");
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
            Self::RunnerLint => runner(&["ruff", "check"]),
            Self::RunnerFormat => runner(&["ruff", "format", "--check"]),
            Self::RunnerTypes => runner(&["pyright"]),
            Self::RunnerLayers => runner(&["lint-imports"]),
            Self::RunnerTests => runner(&["python", "-m", "unittest", "discover", "-s", "tests"]),
            Self::DocLinks => python(&["scripts/check-doc-links.py"]),
        }
    }
}

/// The steps of `groups` (of every group when empty), in order.
fn steps(groups: &[Group]) -> impl Iterator<Item = Step> {
    Step::ALL
        .into_iter()
        .filter(move |step| groups.is_empty() || groups.contains(&step.group()))
}

/// Runs the steps of `groups` (every step when empty) in order and stops at
/// the first failure. The npm packages are installed only for frontend
/// steps, so the other groups need no Node.js.
pub fn run(groups: &[Group]) -> Result<()> {
    if steps(groups).any(|step| step.group() == Group::Frontend) {
        npm::install_if_stale()?;
    }
    for step in steps(groups) {
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
    fn the_bundle_exists_before_the_tests_and_rust_embed_it() {
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
    fn no_group_is_every_step_and_groups_keep_the_order() {
        assert!(steps(&[]).eq(Step::ALL));
        let selected: Vec<_> = steps(&[Group::Scripts, Group::Rust]).collect();
        assert_eq!(selected.first(), Some(&Step::RustFormat));
        assert_eq!(selected.last(), Some(&Step::DocLinks));
        assert!(selected.iter().all(|step| step.group() != Group::Frontend));
        for &group in Group::value_variants() {
            assert!(steps(&[group]).next().is_some(), "{group:?} is empty");
        }
    }

    #[test]
    fn the_runner_runs_locked_in_its_own_directory() {
        assert_eq!(
            process::display(&Step::RunnerTypes.command()),
            "uv run --locked --directory scripts/acceptance pyright"
        );
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
