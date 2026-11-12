//! Registration with the TV: Luna manifests for the supervisor and worker
//! roles, the webosbrew boot hook, a bounded `ls-control` rescan and a
//! bounded activation. Never replaces files it did not write.
use crate::preferences::read_private;
use serde_json::{Value, json};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// Bound on `ls-control scan-services` and on activation.
pub const CHILD_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_REPLY: usize = 65_536;

pub trait Installer {
    /// Write both manifests and the boot hook, then rescan. Idempotent.
    ///
    /// # Errors
    /// An unrelated file is in the way, a write failed, or the rescan failed.
    fn install(&mut self) -> Result<(), String>;
    /// Ask the bus to start the service and check its reply.
    ///
    /// # Errors
    /// No successful reply within the bound.
    fn activate(&mut self, service: &str) -> Result<(), String>;
    /// Remove the hook and both manifests, then rescan. Settings stay.
    ///
    /// # Errors
    /// An unrelated file is in the way, a removal failed, or the rescan failed.
    fn uninstall(&mut self) -> Result<(), String>;
}

/// Where registration lives. Production paths come from the package identity;
/// tests point every path into a temporary directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// Manifest id prefix, the app id such as `io.prysm.tv`.
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
                "version": "1.0.0",
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
                "version": "1.0.0",
                "roleFiles": [self.service_dir.join("worker.role.json")],
            }),
        )
    }

    fn hook_script(&self) -> String {
        format!(
            "#!/bin/sh\n# Prysm ({}): restore Luna registration at boot.\n{} setup > {} 2>&1 &\n",
            self.id,
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

    fn rescan(&self) -> Result<(), String> {
        let (status, _) = run_bounded(
            Command::new(&self.layout.scanner).arg("scan-services"),
            self.timeout,
            false,
        )?;
        if !status.success() {
            return Err(format!("Luna service rescan failed: {status}"));
        }
        Ok(())
    }

    fn hook(&self, install: bool) -> Result<(), String> {
        let path = &self.layout.hook;
        let script = self.layout.hook_script();
        let failed = |error: io::Error| format!("boot hook {}: {error}", path.display());
        let existing = read_private(path, MAX_REPLY).map_err(failed)?;
        if existing
            .as_deref()
            .is_some_and(|bytes| bytes != script.as_bytes())
        {
            return Err(format!(
                "refusing to change an unrelated boot hook: {}",
                path.display()
            ));
        }
        match (install, existing.is_some()) {
            (true, false) => {
                let parent = path.parent().ok_or("boot hook has no directory")?;
                fs::create_dir_all(parent).map_err(failed)?;
                let mut file = tempfile::NamedTempFile::new_in(parent).map_err(failed)?;
                file.write_all(script.as_bytes()).map_err(failed)?;
                file.as_file()
                    .set_permissions(fs::Permissions::from_mode(0o755))
                    .map_err(failed)?;
                file.as_file().sync_all().map_err(failed)?;
                file.persist_noclobber(path)
                    .map_err(|error| failed(error.error))?;
                sync_directory(parent).map_err(failed)
            }
            (false, true) => {
                fs::remove_file(path).map_err(failed)?;
                path.parent()
                    .map_or(Ok(()), |parent| sync_directory(parent).map_err(failed))
            }
            _ => Ok(()),
        }
    }
}

impl Installer for FsInstaller {
    fn install(&mut self) -> Result<(), String> {
        let (path, value) = self.layout.supervisor_manifest();
        manifest(&path, &value, true)?;
        let (path, value) = self.layout.worker_manifest();
        manifest(&path, &value, true)?;
        self.hook(true)?;
        self.rescan()
    }

    fn activate(&mut self, service: &str) -> Result<(), String> {
        let (status, stdout) = run_bounded(
            Command::new(&self.layout.luna_send)
                .args(["-w", "5000", "-n", "1"])
                .arg(format!("luna://{service}/getState"))
                .arg("{}"),
            self.timeout,
            true,
        )?;
        if !status.success() {
            return Err(format!("Luna service activation failed: {status}"));
        }
        activation_reply(&stdout)
    }

    fn uninstall(&mut self) -> Result<(), String> {
        self.hook(false)?;
        let (path, value) = self.layout.worker_manifest();
        manifest(&path, &value, false)?;
        let (path, value) = self.layout.supervisor_manifest();
        manifest(&path, &value, false)?;
        self.rescan()
    }
}

/// Install (create if missing) or remove (if ours) one manifest.
fn manifest(path: &Path, value: &Value, install: bool) -> Result<(), String> {
    let failed = |error: io::Error| format!("manifest {}: {error}", path.display());
    match read_private(path, MAX_REPLY).map_err(failed)? {
        Some(bytes) => {
            if serde_json::from_slice::<Value>(&bytes).ok().as_ref() != Some(value) {
                return Err(format!(
                    "refusing to change an unrelated Luna manifest: {}",
                    path.display()
                ));
            }
            if !install {
                fs::remove_file(path).map_err(failed)?;
            }
        }
        None if install => {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o644)
                .open(path)
                .map_err(failed)?;
            let mut bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
            bytes.push(b'\n');
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(failed)?;
        }
        None => {}
    }
    Ok(())
}

fn sync_directory(path: &Path) -> io::Result<()> {
    fs::File::open(path)?.sync_all()
}

/// Run a child with stdin held open (luna-send exits without replying on
/// EOF), kill it after `timeout`, and optionally keep up to 64 KiB of stdout.
fn run_bounded(
    command: &mut Command,
    timeout: Duration,
    keep_stdout: bool,
) -> Result<(ExitStatus, Vec<u8>), String> {
    let name = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(if keep_stdout {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("cannot run {name}: {error}"))?;
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
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{name} did not finish within {timeout:?}"));
            }
            Err(error) => return Err(format!("cannot wait for {name}: {error}")),
        }
    };
    drop(stdin);
    let stdout = reader
        .map(|reader| reader.join().unwrap_or_default())
        .unwrap_or_default();
    Ok((status, stdout))
}

fn activation_reply(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_REPLY {
        return Err("oversized Luna activation reply".into());
    }
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid Luna activation reply: {error}"))?;
    if value["returnValue"] != true {
        return Err(format!("Luna service activation failed: {value}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
