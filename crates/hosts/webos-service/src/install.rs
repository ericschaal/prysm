//! Registration with the TV: Luna manifests for the supervisor and worker
//! roles, the webosbrew boot hook, a bounded `ls-control` rescan and a
//! bounded activation. Never replaces files it did not write.
use crate::preferences::{read_private, write_atomic};
use serde_json::{Value, json};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// Bound on `ls-control scan-services` and on activation.
pub const CHILD_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_REPLY: usize = 65_536;

/// The Luna manifests' version: the product version's major.minor.patch,
/// the same rule as the packaged appinfo.json (`ipk.py app_version`), which
/// takes no pre-release suffix.
pub const MANIFEST_VERSION: &str = prysm_version::RELEASE;

/// Why registration could not be changed.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("boot hook {}", path.display())]
    Hook {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("refusing to change an unrelated boot hook: {}", .0.display())]
    UnrelatedHook(PathBuf),
    #[error("manifest {}", path.display())]
    Manifest {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("refusing to change an unrelated Luna manifest: {}", .0.display())]
    UnrelatedManifest(PathBuf),
    #[error(
        "{} is not a Luna manifest (empty, cut short or not JSON): delete it, then run setup again",
        path.display()
    )]
    InvalidManifest {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("cannot run {program}")]
    Spawn {
        program: String,
        #[source]
        source: io::Error,
    },
    #[error("cannot wait for {program}")]
    Wait {
        program: String,
        #[source]
        source: io::Error,
    },
    #[error("{program} did not finish within {timeout:?}")]
    Timeout { program: String, timeout: Duration },
    #[error("Luna service rescan failed: {0}")]
    Rescan(ExitStatus),
    #[error("Luna service activation failed: {0}")]
    Activation(ExitStatus),
    #[error("Luna service activation failed: {0}")]
    ActivationRefused(Value),
    #[error("invalid Luna activation reply")]
    InvalidReply(#[source] serde_json::Error),
    #[error("oversized Luna activation reply")]
    OversizedReply,
}

type Result<T> = std::result::Result<T, InstallError>;

pub trait Installer {
    /// Write both manifests and the boot hook, then rescan, also after a
    /// failed step (whatever was changed takes effect). Idempotent.
    ///
    /// # Errors
    /// An unrelated file is in the way, a write failed, or the rescan failed.
    fn install(&mut self) -> Result<()>;
    /// Ask the bus to start the service and check its reply.
    ///
    /// # Errors
    /// No successful reply within the bound.
    fn activate(&mut self, service: &str) -> Result<()>;
    /// Remove the hook and both manifests, then rescan, also after a failed
    /// step. Settings stay.
    ///
    /// # Errors
    /// An unrelated file is in the way, a removal failed, or the rescan failed.
    fn uninstall(&mut self) -> Result<()>;
}

/// Where registration lives. Production paths come from the package identity;
/// tests point every path into a temporary directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// Manifest id prefix, the app id (`identity::APP_ID` on a TV).
    pub id: String,
    /// `/var/luna-service2/manifests.d`.
    pub manifests: PathBuf,
    /// The installed service directory with role, service and permission files.
    pub service_dir: PathBuf,
    /// The supervisor binary the boot hook runs with `setup`.
    pub supervisor: PathBuf,
    /// `/var/lib/webosbrew/init.d/<id>`.
    pub hook: PathBuf,
    /// `/usr/sbin/ls-control`.
    pub scanner: PathBuf,
    /// `/usr/bin/luna-send`.
    pub luna_send: PathBuf,
    /// Where the boot hook's setup output goes.
    pub hook_log: PathBuf,
}

impl Layout {
    fn supervisor_manifest(&self) -> (PathBuf, Value) {
        let file = |name: &str| self.service_dir.join(name);
        (
            self.manifests.join(format!("{}.supervisor.json", self.id)),
            json!({
                "id": format!("{}.supervisor", self.id),
                "version": MANIFEST_VERSION,
                "roleFiles": [file("supervisor.role.json")],
                "serviceFiles": [file("supervisor.service")],
                "clientPermissionFiles": [file("supervisor.client.json")],
                "apiPermissionFiles": [file("supervisor.api.json")],
            }),
        )
    }

    fn worker_manifest(&self) -> (PathBuf, Value) {
        (
            self.manifests.join(format!("{}.worker.json", self.id)),
            json!({
                "id": format!("{}.worker", self.id),
                "version": MANIFEST_VERSION,
                "roleFiles": [self.service_dir.join("worker.role.json")],
            }),
        )
    }

    /// The start of the boot hook's comment line, which marks a hook as
    /// ours whatever release wrote the rest.
    fn hook_marker(&self) -> String {
        format!("# Prysm ({}):", self.id)
    }

    fn hook_script(&self) -> String {
        format!(
            "#!/bin/sh\n{} restore Luna registration at boot.\n{} setup > {} 2>&1 &\n",
            self.hook_marker(),
            quote(&self.supervisor),
            quote(&self.hook_log),
        )
    }
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

/// The real installer.
#[derive(Debug, Clone)]
pub struct FsInstaller {
    pub layout: Layout,
    pub timeout: Duration,
}

impl FsInstaller {
    #[must_use]
    pub fn new(layout: Layout) -> Self {
        Self {
            layout,
            timeout: CHILD_TIMEOUT,
        }
    }

    fn rescan(&self) -> Result<()> {
        let Finished { status, .. } = run_bounded(
            Command::new(&self.layout.scanner).arg("scan-services"),
            self.timeout,
            false,
        )?;
        if !status.success() {
            return Err(InstallError::Rescan(status));
        }
        Ok(())
    }

    fn hook(&self, install: bool) -> Result<()> {
        let path = &self.layout.hook;
        let script = self.layout.hook_script();
        let failed = |source: io::Error| InstallError::Hook {
            path: path.clone(),
            source,
        };
        let existing = read_private(path, MAX_REPLY).map_err(failed)?;
        let marker = self.layout.hook_marker();
        if let Some(bytes) = &existing
            && !bytes
                .split(|byte| *byte == b'\n')
                .any(|line| line.starts_with(marker.as_bytes()))
        {
            return Err(InstallError::UnrelatedHook(path.clone()));
        }
        match (install, existing) {
            (true, Some(bytes)) if bytes == script.as_bytes() => Ok(()),
            (true, existing) => {
                let parent = path
                    .parent()
                    .ok_or_else(|| failed(io::Error::other("no directory")))?;
                fs::create_dir_all(parent).map_err(failed)?;
                write_atomic(path, script.as_bytes(), 0o755, existing.is_some()).map_err(failed)
            }
            (false, Some(_)) => remove(path).map_err(failed),
            (false, None) => Ok(()),
        }
    }

    fn register(&self) -> Result<()> {
        let (path, value) = self.layout.supervisor_manifest();
        manifest(&path, &value, true)?;
        let (path, value) = self.layout.worker_manifest();
        manifest(&path, &value, true)?;
        self.hook(true)
    }

    fn deregister(&self) -> Result<()> {
        self.hook(false)?;
        let (path, value) = self.layout.worker_manifest();
        manifest(&path, &value, false)?;
        let (path, value) = self.layout.supervisor_manifest();
        manifest(&path, &value, false)
    }
}

impl Installer for FsInstaller {
    fn install(&mut self) -> Result<()> {
        let registered = self.register();
        let rescanned = self.rescan();
        registered.and(rescanned)
    }

    fn activate(&mut self, service: &str) -> Result<()> {
        let Finished { status, stdout } = run_bounded(
            Command::new(&self.layout.luna_send)
                .args(["-w", "5000", "-n", "1"])
                .arg(format!("luna://{service}/getState"))
                .arg("{}"),
            self.timeout,
            true,
        )?;
        if !status.success() {
            return Err(InstallError::Activation(status));
        }
        activation_reply(&stdout)
    }

    fn uninstall(&mut self) -> Result<()> {
        let deregistered = self.deregister();
        let rescanned = self.rescan();
        deregistered.and(rescanned)
    }
}

/// Install (create, or replace ours if it differs) or remove (if ours) one
/// manifest.
///
/// Ours is a manifest with our `id` (the app id's role) and our `roleFiles`
/// (in our installed service directory). Its version and other lists may be
/// another release's: an update that did not run `setup` leaves the old
/// release's manifest behind.
fn manifest(path: &Path, value: &Value, install: bool) -> Result<()> {
    let failed = |source: io::Error| InstallError::Manifest {
        path: path.to_owned(),
        source,
    };
    let existing = read_private(path, MAX_REPLY)
        .map_err(failed)?
        .map(|bytes| serde_json::from_slice::<Value>(&bytes))
        .transpose()
        .map_err(|source| InstallError::InvalidManifest {
            path: path.to_owned(),
            source,
        })?;
    if let Some(existing) = &existing
        && (existing["id"] != value["id"] || existing["roleFiles"] != value["roleFiles"])
    {
        return Err(InstallError::UnrelatedManifest(path.to_owned()));
    }
    match (install, existing) {
        (true, Some(existing)) if existing == *value => Ok(()),
        (true, existing) => {
            let mut bytes = serde_json::to_vec(value)
                .map_err(io::Error::from)
                .map_err(failed)?;
            bytes.push(b'\n');
            write_atomic(path, &bytes, 0o644, existing.is_some()).map_err(failed)
        }
        (false, Some(_)) => remove(path).map_err(failed),
        (false, None) => Ok(()),
    }
}

/// Remove `path` and sync its directory.
fn remove(path: &Path) -> io::Result<()> {
    fs::remove_file(path)?;
    let directory = path.parent().unwrap_or(Path::new("."));
    fs::File::open(directory)?.sync_all()
}

/// How a bounded child ended.
struct Finished {
    status: ExitStatus,
    /// Up to 64 KiB of its stdout when kept, else empty.
    stdout: Vec<u8>,
}

/// Run a child with stdin held open (luna-send exits without replying on
/// EOF), kill it after `timeout`, and optionally keep up to 64 KiB of stdout.
/// A child that is not waited for to the end (a timeout, or `waitpid`
/// failing) is killed and reaped before the error returns.
fn run_bounded(command: &mut Command, timeout: Duration, keep_stdout: bool) -> Result<Finished> {
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(if keep_stdout {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|source| InstallError::Spawn {
            program: program.clone(),
            source,
        })?;
    let stdin = child.stdin.take();
    let reader = child.stdout.take().map(|stdout| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.take(MAX_REPLY as u64 + 1).read_to_end(&mut bytes);
            bytes
        })
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                kill_and_reap(&mut child, &program);
                return Err(InstallError::Timeout { program, timeout });
            }
            Err(source) => {
                kill_and_reap(&mut child, &program);
                return Err(InstallError::Wait { program, source });
            }
        }
    };
    drop(stdin);
    let stdout = reader
        .map(|reader| reader.join().unwrap_or_default())
        .unwrap_or_default();
    Ok(Finished { status, stdout })
}

/// Stop a child that is given up on. A kill that fails (it already exited)
/// leaves the reap; a failed reap is reported, the caller's error stands.
fn kill_and_reap(child: &mut Child, program: &str) {
    let _ = child.kill();
    if let Err(error) = child.wait() {
        eprintln!("prysm-webos-service: cannot reap {program}: {error}");
    }
}

fn activation_reply(bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_REPLY {
        return Err(InstallError::OversizedReply);
    }
    let value: Value = serde_json::from_slice(bytes).map_err(InstallError::InvalidReply)?;
    if value["returnValue"] != true {
        return Err(InstallError::ActivationRefused(value));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
