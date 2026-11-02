//! Private state persistence, Luna registration, and boot-hook ownership.
use crate::config::Config;
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub struct Files {
    pub root: PathBuf,
    pub worker: PathBuf,
    pub scanner: PathBuf,
    pub app_id: String,
    manifest_path: PathBuf,
    capture_role: PathBuf,
    supervisor_manifest_path: PathBuf,
    hook_path: PathBuf,
    boot_script: String,
}
impl Files {
    pub fn production() -> Result<Self> {
        let root = PathBuf::from("/var/lib/prysm-capture");
        match fs::DirBuilder::new().mode(0o700).create(&root) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error.into()),
        }
        let metadata = fs::symlink_metadata(&root)?;
        ensure!(
            metadata.is_dir()
                && metadata.uid() == 0
                && metadata.permissions().mode() & 0o777 == 0o700,
            "Capture state directory must be root-owned and private (0700)"
        );
        let service =
            PathBuf::from("/media/developer/apps/usr/palm/services/io.prysm.capture.service");
        Ok(Self {
            worker: service.join("prysm-webos-capture"),
            scanner: PathBuf::from("/usr/sbin/ls-control"),
            app_id: "io.prysm.capture".into(),
            manifest_path: PathBuf::from(
                "/var/luna-service2/manifests.d/io.prysm.capture.native.json",
            ),
            capture_role: service.join("capture.role.json"),
            supervisor_manifest_path: PathBuf::from(
                "/var/luna-service2/manifests.d/io.prysm.capture.supervisor.native.json",
            ),
            hook_path: PathBuf::from("/var/lib/webosbrew/init.d/prysm-capture"),
            boot_script: format!(
                "#!/bin/sh\nif [ -x {0}/setup.sh ]; then\n  {0}/setup.sh > /tmp/prysm-capture-setup.log 2>&1 &\nfi\n",
                service.display()
            ),
            root,
        })
    }
    /// Sandbox files never reference the installed app, manifests, or boot hooks.
    pub fn sandbox(root: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
        let temporary = std::env::temp_dir().canonicalize()?;
        ensure!(
            root.starts_with(temporary)
                && root
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("prysm-supervisor.")),
            "Sandbox must be a prysm-supervisor.* directory under /tmp"
        );
        ensure!(
            fs::metadata(&root)?.permissions().mode() & 0o777 == 0o700,
            "Sandbox must be private (0700)"
        );
        Ok(Self {
            worker: root.join("worker"),
            scanner: root.join("scan-services"),
            manifest_path: root.join("capture.manifest.json"),
            capture_role: root.join("capture.role.json"),
            supervisor_manifest_path: root.join("supervisor.manifest.json"),
            hook_path: root.join("boot-hook"),
            boot_script: {
                let quoted = format!("'{}'", root.display().to_string().replace('\'', "'\\''"));
                format!(
                    "#!/bin/sh\nif [ -x {quoted}/setup.sh ]; then\n  {quoted}/setup.sh > {quoted}/setup.log 2>&1 &\nfi\n"
                )
            },
            root,
            app_id: "io.prysm.capture.probe.app".into(),
        })
    }
    pub fn load(&self) -> Result<Config> {
        match fs::read(self.root.join("config.json")) {
            Ok(data) => {
                ensure!(data.len() <= 65536, "Oversized capture configuration");
                let value: Config = serde_json::from_slice(&data)?;
                value.validate()?;
                Ok(value)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(error) => Err(error.into()),
        }
    }
    pub fn persist(&self, value: &Config) -> Result<()> {
        let mut file = atomic_write_file::AtomicWriteFile::open(self.root.join("config.json"))?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        serde_json::to_writer(&mut file, value)?;
        file.write_all(b"\n")?;
        file.commit()?;
        fs::File::open(&self.root)?.sync_all()?;
        Ok(())
    }
    pub fn manifest(&self, install: bool) -> Result<()> {
        let value = serde_json::json!({"id":"io.prysm.capture.native", "version":"1.0.0", "roleFiles":[self.capture_role]});
        registration(&self.manifest_path, &value, install)
    }
    pub fn register_supervisor(&self) -> Result<()> {
        let service = self.capture_role.parent().context("service directory")?;
        let value = serde_json::json!({
            "id":"io.prysm.capture.supervisor.native", "version":"1.0.0",
            "roleFiles":[service.join("supervisor.role.json")],
            "serviceFiles":[service.join("supervisor.service")],
            "clientPermissionFiles":[service.join("supervisor.client.json")],
            "apiPermissionFiles":[service.join("supervisor.api.json")]
        });
        registration(&self.supervisor_manifest_path, &value, true)
    }
    pub fn autostart(&self, enabled: bool) -> Result<()> {
        let script = &self.boot_script;
        let path = &self.hook_path;
        match fs::read_to_string(path) {
            Ok(existing) => ensure!(
                existing == *script,
                "Refusing to change an unrelated boot hook"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        if enabled {
            fs::create_dir_all(path.parent().context("boot hook parent")?)?;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o755)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path)?;
            file.write_all(script.as_bytes())?;
        } else {
            Self::remove(path)?;
        }
        Ok(())
    }
    pub fn remove(path: &Path) -> Result<()> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

fn registration(path: &Path, value: &serde_json::Value, install: bool) -> Result<()> {
    match fs::read(path) {
        Ok(bytes) => ensure!(
            serde_json::from_slice::<serde_json::Value>(&bytes)? == *value,
            "Refusing to change an unrelated Luna manifest: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if install {
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o644)
                    .open(path)?;
                serde_json::to_writer(&mut file, value)?;
                file.write_all(b"\n")?;
            }
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    }
    if !install {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore = "filesystem integration runs in ordinary tests")]
    fn persistence_and_permission_files_preserve_unrelated_content() {
        let directory = tempfile::Builder::new()
            .prefix("prysm-supervisor.")
            .tempdir()
            .unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let files = Files::sandbox(directory.path()).unwrap();
        assert_eq!(files.load().unwrap(), Config::default());
        let value = Config {
            receiver: Some("127.0.0.1:19400".parse().unwrap()),
            metrics: true,
            ..Config::default()
        };
        files.persist(&value).unwrap();
        assert_eq!(files.load().unwrap(), value);
        assert_eq!(
            fs::metadata(directory.path().join("config.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        files.register_supervisor().unwrap();
        files.register_supervisor().unwrap();
        let supervisor_path = directory.path().join("supervisor.manifest.json");
        let supervisor: serde_json::Value =
            serde_json::from_slice(&fs::read(&supervisor_path).unwrap()).unwrap();
        assert_eq!(
            supervisor["roleFiles"][0],
            files.root.join("supervisor.role.json").to_str().unwrap()
        );
        files.manifest(true).unwrap();
        files.manifest(true).unwrap();
        files.manifest(false).unwrap();
        files.manifest(false).unwrap();
        assert!(
            supervisor_path.exists(),
            "capture cleanup must leave the supervisor activatable"
        );
        fs::write(&supervisor_path, "{\"id\":\"unrelated\"}").unwrap();
        assert!(files.register_supervisor().is_err());
        assert_eq!(
            fs::read_to_string(supervisor_path).unwrap(),
            "{\"id\":\"unrelated\"}"
        );
        files.autostart(true).unwrap();
        files.autostart(false).unwrap();
        let manifest = directory.path().join("capture.manifest.json");
        fs::write(&manifest, "{\"id\":\"unrelated\"}").unwrap();
        assert!(files.manifest(true).is_err());
        assert!(files.manifest(false).is_err());
        let hook = directory.path().join("boot-hook");
        fs::write(&hook, "unrelated").unwrap();
        assert!(files.autostart(true).is_err());
        assert!(files.autostart(false).is_err());
        assert_eq!(fs::read_to_string(hook).unwrap(), "unrelated");
    }
}
