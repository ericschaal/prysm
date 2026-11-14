//! The supervisor process: arguments, private directories, signals.
use crate::identity::Identity;
use crate::install::{FsInstaller, Installer};
use crate::state::Flavor;
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use prysm_ports::chain;
use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

/// The TV supervisor of Prysm.
#[derive(Debug, Parser)]
#[command(
    version = prysm_version::VERSION,
    args_conflicts_with_subcommands = true,
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    action: Option<Action>,
    #[command(flatten)]
    run: RunArgs,
}

#[derive(Debug, Subcommand)]
enum Action {
    /// Register the Luna service and supervise the worker (Luna activation; the default)
    Run(RunArgs),
    /// Write the Luna manifests and boot hook, rescan, activate (setup.sh, boot hook)
    Setup(Sandbox),
    /// Remove the boot hook and manifests and rescan; preferences stay
    Uninstall(Sandbox),
}

#[derive(Debug, Args)]
struct Sandbox {
    /// Keep every file in DIR, a private prysm-supervisor.* temporary directory, with a probe app and service name
    #[arg(long, value_name = "DIR")]
    sandbox: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct RunArgs {
    /// Keep every file in DIR, a private prysm-supervisor.* temporary directory, with a probe app; needs --run-for
    #[arg(long, value_name = "DIR", requires = "run_for")]
    sandbox: Option<PathBuf>,
    /// Stop a sandboxed run after SECONDS (1 to 300)
    #[arg(long, value_name = "SECONDS", requires = "sandbox", value_parser = clap::value_parser!(u64).range(1..=300))]
    run_for: Option<u64>,
    /// Luna service name of a sandboxed run
    #[arg(long, value_name = "NAME", requires = "sandbox")]
    service_name: Option<String>,
}

/// The process entry point of both supervisor binaries.
pub fn main(flavor: Flavor) -> ExitCode {
    let name = match flavor {
        Flavor::Full => "prysm-webos-service",
        Flavor::Slim => "prysm-webos-slim-service",
    };
    let cli = Cli::command().name(name).bin_name(name).get_matches();
    let cli = Cli::from_arg_matches(&cli).unwrap_or_else(|error| error.exit());
    match run(flavor, cli.action.unwrap_or(Action::Run(cli.run))) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{name}: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(flavor: Flavor, action: Action) -> Result<(), String> {
    let (sandbox, service_name) = match &action {
        Action::Run(run) => (&run.sandbox, run.service_name.clone()),
        Action::Setup(setup) | Action::Uninstall(setup) => (&setup.sandbox, None),
    };
    let identity = match sandbox {
        Some(root) => {
            let root = sandbox_root(root)?;
            let binaries = std::env::current_exe()
                .map_err(|error| format!("cannot locate this binary: {error}"))?;
            let binaries = binaries.parent().unwrap_or(Path::new("/"));
            Identity::sandbox(flavor, &root, binaries, service_name)
        }
        None => {
            if !nix::unistd::Uid::effective().is_root() {
                return Err("the TV service requires root".into());
            }
            Identity::production(flavor)
        }
    };
    let mut installer = FsInstaller::new(identity.layout.clone());
    match &action {
        Action::Setup(_) => {
            installer.install().map_err(|error| chain(&error))?;
            installer
                .activate(&identity.service)
                .map_err(|error| chain(&error))?;
            println!("{} is registered and running", identity.service);
            Ok(())
        }
        Action::Uninstall(_) => installer.uninstall().map_err(|error| chain(&error)),
        Action::Run(run) => {
            private_dir(&identity.state_dir, sandbox.is_none())?;
            let stop = Arc::new(AtomicBool::new(false));
            for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
                signal_hook::flag::register(signal, stop.clone())
                    .map_err(|error| format!("cannot install signal handlers: {error}"))?;
            }
            let deadline = run
                .run_for
                .map(|seconds| Instant::now() + Duration::from_secs(seconds));
            crate::luna::serve(&identity, stop, deadline).map_err(|error| chain(&error))
        }
    }
}

/// A sandbox is a private `prysm-supervisor.*` directory under the
/// temporary directory, so a probe can never touch installed files.
fn sandbox_root(root: &Path) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("sandbox {}: {error}", root.display()))?;
    let temporary = std::env::temp_dir()
        .canonicalize()
        .map_err(|error| format!("temporary directory: {error}"))?;
    let named = root
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with("prysm-supervisor."));
    if !root.starts_with(&temporary) || !named {
        return Err(
            "the sandbox must be a prysm-supervisor.* directory in the temporary directory".into(),
        );
    }
    let mode = fs::metadata(&root)
        .map_err(|error| format!("sandbox: {error}"))?
        .permissions()
        .mode();
    if mode & 0o777 != 0o700 {
        return Err("the sandbox must be private (0700)".into());
    }
    Ok(root)
}

/// Create (0700) or check a private state directory.
fn private_dir(path: &Path, require_root: bool) -> Result<(), String> {
    let failed = |error: std::io::Error| format!("state directory {}: {error}", path.display());
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(failed(error)),
    }
    let metadata = fs::symlink_metadata(path).map_err(failed)?;
    let private = metadata.is_dir() && metadata.permissions().mode() & 0o777 == 0o700;
    if !private || (require_root && metadata.uid() != 0) {
        return Err(format!(
            "state directory {} must be a private (0700){} directory",
            path.display(),
            if require_root { " root-owned" } else { "" }
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Action, clap::Error> {
        let cli = Cli::try_parse_from(["prysm"].iter().chain(args))?;
        Ok(cli.action.unwrap_or(Action::Run(cli.run)))
    }

    #[test]
    fn arguments() {
        assert!(matches!(parse(&[]).unwrap(), Action::Run(_)));
        assert!(matches!(parse(&["setup"]).unwrap(), Action::Setup(_)));
        let Action::Run(probe) = parse(&[
            "run",
            "--sandbox",
            "/tmp/prysm-supervisor.x",
            "--run-for",
            "30",
            "--service-name",
            "a.b",
        ])
        .unwrap() else {
            panic!("expected run");
        };
        assert_eq!(probe.run_for, Some(30));
        assert_eq!(probe.service_name.as_deref(), Some("a.b"));
        assert!(matches!(
            parse(&["uninstall", "--sandbox", "/tmp/x"]).unwrap(),
            Action::Uninstall(_)
        ));
        for bad in [
            &["--run-for", "30"][..],
            &["--sandbox", "/tmp/x"],
            &["--sandbox", "/tmp/x", "--run-for", "301"],
            &["--sandbox", "/tmp/x", "--run-for", "0"],
            &["setup", "--sandbox", "/tmp/x", "--run-for", "5"],
            &["start"],
            &["--sandbox"],
            &["--sandbox", "a", "--sandbox", "b", "--run-for", "5"],
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn sandbox_must_be_a_private_named_temporary_directory() {
        let named = tempfile::Builder::new()
            .prefix("prysm-supervisor.")
            .tempdir()
            .unwrap();
        fs::set_permissions(named.path(), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(sandbox_root(named.path()).is_ok());
        fs::set_permissions(named.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(sandbox_root(named.path()).unwrap_err().contains("0700"));
        let other = tempfile::tempdir().unwrap();
        fs::set_permissions(other.path(), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(sandbox_root(other.path()).is_err());
    }

    #[test]
    fn state_directory_is_created_private_and_checked() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        private_dir(&state, false).unwrap();
        assert_eq!(
            fs::metadata(&state).unwrap().permissions().mode() & 0o777,
            0o700
        );
        private_dir(&state, false).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_dir(&state, false).is_err());
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&state, &link).unwrap();
        assert!(private_dir(&link, false).is_err(), "links are refused");
    }
}
