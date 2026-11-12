//! The supervisor process: arguments, private directories, signals.
use crate::identity::Identity;
use crate::install::{FsInstaller, Installer};
use crate::state::Flavor;
use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

const USAGE: &str = "[run] [--sandbox DIR --run-for SECONDS [--service-name NAME]]
       setup [--sandbox DIR]
       uninstall [--sandbox DIR]

run        Register the Luna service and supervise the worker (Luna activation).
setup      Write the Luna manifests and boot hook, rescan, activate (setup.sh, boot hook).
uninstall  Remove the boot hook and manifests and rescan; preferences stay.
--sandbox  Keep every file in DIR (a private prysm-supervisor.* directory under
           the temporary directory) with a probe app and service name; `run`
           then also needs --run-for (1 to 300 seconds).";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Run,
    Setup,
    Uninstall,
}

#[derive(Debug, PartialEq, Eq)]
struct Cli {
    action: Action,
    sandbox: Option<PathBuf>,
    run_for: Option<Duration>,
    service_name: Option<String>,
}

fn parse(args: &[String]) -> Result<Cli, String> {
    let mut args = args.iter().peekable();
    let action = match args.peek().map(|arg| arg.as_str()) {
        Some("run") => Action::Run,
        Some("setup") => Action::Setup,
        Some("uninstall") => Action::Uninstall,
        _ => {
            return parse_options(Action::Run, args);
        }
    };
    args.next();
    parse_options(action, args)
}

fn parse_options<'a>(
    action: Action,
    mut args: impl Iterator<Item = &'a String>,
) -> Result<Cli, String> {
    let mut cli = Cli {
        action,
        sandbox: None,
        run_for: None,
        service_name: None,
    };
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--sandbox" if cli.sandbox.is_none() => cli.sandbox = Some(value()?.into()),
            "--run-for" if cli.run_for.is_none() => {
                let seconds: u64 = value()?
                    .parse()
                    .map_err(|_| "--run-for needs whole seconds".to_owned())?;
                if !(1..=300).contains(&seconds) {
                    return Err("--run-for must be 1 to 300 seconds".into());
                }
                cli.run_for = Some(Duration::from_secs(seconds));
            }
            "--service-name" if cli.service_name.is_none() => cli.service_name = Some(value()?),
            _ => return Err(format!("unexpected argument `{arg}`")),
        }
    }
    let probe = cli.run_for.is_some() || cli.service_name.is_some();
    if probe && (cli.action != Action::Run || cli.sandbox.is_none()) {
        return Err("--run-for and --service-name are for `run --sandbox`".into());
    }
    if cli.action == Action::Run && cli.sandbox.is_some() && cli.run_for.is_none() {
        return Err("a sandboxed run needs --run-for".into());
    }
    Ok(cli)
}

/// The process entry point of both supervisor binaries.
pub fn main(flavor: Flavor) -> ExitCode {
    let name = match flavor {
        Flavor::Full => "prysm-webos-service",
        Flavor::Slim => "prysm-webos-slim-service",
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("usage: {name} {USAGE}");
        return ExitCode::SUCCESS;
    }
    let cli = match parse(&args) {
        Ok(cli) => cli,
        Err(message) => {
            eprintln!("{name}: {message}\n\nusage: {name} {USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(flavor, &cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{name}: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(flavor: Flavor, cli: &Cli) -> Result<(), String> {
    let identity = match &cli.sandbox {
        Some(root) => {
            let root = sandbox_root(root)?;
            let binaries = std::env::current_exe()
                .map_err(|error| format!("cannot locate this binary: {error}"))?;
            let binaries = binaries.parent().unwrap_or(Path::new("/"));
            Identity::sandbox(flavor, &root, binaries, cli.service_name.clone())
        }
        None => {
            if !nix::unistd::Uid::effective().is_root() {
                return Err("the TV service requires root".into());
            }
            Identity::production(flavor)
        }
    };
    let mut installer = FsInstaller::new(identity.layout.clone());
    match cli.action {
        Action::Setup => {
            installer.install()?;
            installer.activate(&identity.service)?;
            println!("{} is registered and running", identity.service);
            Ok(())
        }
        Action::Uninstall => installer.uninstall(),
        Action::Run => {
            private_dir(&identity.state_dir, cli.sandbox.is_none())?;
            let stop = Arc::new(AtomicBool::new(false));
            for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
                signal_hook::flag::register(signal, stop.clone())
                    .map_err(|error| format!("cannot install signal handlers: {error}"))?;
            }
            let deadline = cli.run_for.map(|lifetime| Instant::now() + lifetime);
            crate::luna::serve(&identity, stop, deadline)
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

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn arguments() {
        let run = parse(&[]).unwrap();
        assert_eq!(run.action, Action::Run);
        assert_eq!(parse(&strings(&["setup"])).unwrap().action, Action::Setup);
        let probe = parse(&strings(&[
            "run",
            "--sandbox",
            "/tmp/prysm-supervisor.x",
            "--run-for",
            "30",
            "--service-name",
            "a.b",
        ]))
        .unwrap();
        assert_eq!(probe.run_for, Some(Duration::from_secs(30)));
        assert_eq!(probe.service_name.as_deref(), Some("a.b"));
        assert_eq!(
            parse(&strings(&["uninstall", "--sandbox", "/tmp/x"]))
                .unwrap()
                .action,
            Action::Uninstall
        );
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
            assert!(parse(&strings(bad)).is_err(), "{bad:?}");
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
