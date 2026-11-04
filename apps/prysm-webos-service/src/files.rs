//! Private state persistence, Luna registration, and boot-hook ownership.
use crate::config::{Config, ProcessingLocation};
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

const MAX_CONFIG_BYTES: usize = 65_536;

fn read_file(path: &Path, limit: usize) -> Result<Option<Vec<u8>>> {
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("Open {}", path.display())),
    };
    ensure!(
        file.metadata()?.is_file(),
        "Expected file: {}",
        path.display()
    );
    let mut data = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut data)?;
    ensure!(data.len() <= limit, "Oversized file: {}", path.display());
    Ok(Some(data))
}

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
        let current = self.root.join("config-v3.json");
        if let Some(data) = read_file(&current, MAX_CONFIG_BYTES)? {
            return serde_json::from_slice::<Config>(&data)?.restore();
        }
        if let Some(data) = read_file(&self.root.join("config-v2.json"), MAX_CONFIG_BYTES)? {
            // The portal owns lighting settings. Only this on-disk migration accepts embeddedOutput.
            let mut saved: serde_json::Value = serde_json::from_slice(&data)?;
            let output = saved
                .as_object_mut()
                .and_then(|o| o.remove("embeddedOutput"));
            let config: Config = serde_json::from_value(saved)?;
            config.clone().restore()?;
            if let Some(output) = output.filter(|o| !o.is_null()) {
                self.migrate_output(output)?;
            }
            // Publish the migration marker last. A crash before this point can retry
            // without overwriting the portal store or changing the v2 rollback snapshot.
            self.create_once(&current, &config)?;
            let data =
                read_file(&current, MAX_CONFIG_BYTES)?.context("missing migrated preferences")?;
            return serde_json::from_slice::<Config>(&data)?.restore();
        }
        match read_file(&self.root.join("config.json"), MAX_CONFIG_BYTES)? {
            Some(data) => serde_json::from_slice::<Config>(&data)?.restore(),
            None => Ok(Config::default()),
        }
    }

    pub fn portal_settings_path(&self) -> PathBuf {
        self.root.join("portal-settings.json")
    }

    fn migrate_output(&self, legacy: serde_json::Value) -> Result<()> {
        use prysm_contract::{Input, Settings};
        let path = self.portal_settings_path();
        match fs::symlink_metadata(&path) {
            // An existing store, including a damaged one, belongs to the portal.
            Ok(_) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
        let settings = Settings {
            input: Some(Input::Webos {
                capture_resolution: [1280, 720].try_into().map_err(anyhow::Error::msg)?,
                refresh_rate_hz: 30.try_into()?,
            }),
            output: Some(serde_json::from_value(legacy)?),
            ..Settings::default()
        };
        settings.validate()?;
        self.create_once(&path, &settings)
    }

    fn create_once(&self, path: &Path, value: &impl serde::Serialize) -> Result<()> {
        let mut file = tempfile::NamedTempFile::new_in(&self.root)?;
        serde_json::to_writer_pretty(&mut file, value)?;
        file.write_all(b"\n")?;
        file.as_file().sync_all()?;
        match file.persist_noclobber(path) {
            Ok(_) => Ok(fs::File::open(&self.root)?.sync_all()?),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(error.error.into()),
        }
    }

    pub fn has_versioned_config(&self) -> Result<bool> {
        Ok(self.root.join("config-v3.json").try_exists()?)
    }

    pub fn persist(&self, value: &Config) -> Result<()> {
        // Older services continue to see their last readable preference snapshot.
        let versioned = self.root.join("config-v3.json");
        let extended = read_file(&versioned, MAX_CONFIG_BYTES)?;
        let (path, previous) =
            if extended.is_some() || value.location == ProcessingLocation::Embedded {
                (versioned, extended)
            } else {
                let path = self.root.join("config.json");
                let previous = read_file(&path, MAX_CONFIG_BYTES)?;
                (path, previous)
            };
        let mut data = serde_json::to_vec(value)?;
        data.push(b'\n');
        ensure!(data.len() <= MAX_CONFIG_BYTES, "Oversized configuration");
        let file = Self::stage_config(&path, &data)?;
        let had_hook = self.has_boot_hook()?;
        if let Err(error) =
            self.commit_with_autostart(value.needs_boot_hook(), || Ok(file.commit()?))
        {
            self.commit_with_autostart(had_hook, || {
                self.restore_config(&path, previous.as_deref())
            })
            .with_context(|| format!("Rollback after settings save failed: {error}"))?;
            return Err(error);
        }
        Ok(())
    }
    fn commit_with_autostart(
        &self,
        enabled: bool,
        commit: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        // Any saved mode that needs activation must remain bootable after power loss.
        if enabled {
            self.autostart(true)?;
        }
        commit()?;
        if !enabled {
            self.autostart(false)?;
        }
        Ok(())
    }
    fn stage_config(path: &Path, data: &[u8]) -> Result<atomic_write_file::AtomicWriteFile> {
        let mut file = atomic_write_file::AtomicWriteFile::open(path)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(data)?;
        Ok(file)
    }
    fn restore_config(&self, path: &Path, previous: Option<&[u8]>) -> Result<()> {
        // commit can report a directory sync error after the rename has already succeeded.
        if read_file(path, MAX_CONFIG_BYTES)?.as_deref() != previous {
            if let Some(data) = previous {
                Self::stage_config(path, data)?.commit()?;
            } else {
                Self::remove(path)?;
                fs::File::open(&self.root)?.sync_all()?;
            }
        }
        Ok(())
    }
    pub fn manifest(&self, install: bool) -> Result<()> {
        let value = serde_json::json!({"id":"io.prysm.capture.native", "version":"1.0.0", "roleFiles":[self.capture_role]});
        registration(&self.manifest_path, &value, install)?;
        // Its own identity lets a standard build remove the role on downgrade.
        registration(
            &self
                .manifest_path
                .with_file_name("io.prysm.capture.embedded.native.json"),
            &serde_json::json!({"id":"io.prysm.capture.embedded.native","version":"1.0.0",
                "roleFiles":[self.capture_role.with_file_name("embedded.role.json")]}),
            install && cfg!(feature = "embedded-prysm"),
        )
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
    fn has_boot_hook(&self) -> Result<bool> {
        let existing = read_file(&self.hook_path, self.boot_script.len())?;
        ensure!(
            existing
                .as_deref()
                .is_none_or(|bytes| bytes == self.boot_script.as_bytes()),
            "Refusing to change an unrelated boot hook"
        );
        Ok(existing.is_some())
    }
    pub fn autostart(&self, enabled: bool) -> Result<()> {
        let exists = self.has_boot_hook()?;
        if !enabled && !exists {
            return Ok(());
        }
        let path = &self.hook_path;
        let parent = path.parent().context("boot hook parent")?;
        if enabled && !exists {
            fs::create_dir_all(parent)?;
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            file.write_all(self.boot_script.as_bytes())?;
            file.as_file()
                .set_permissions(fs::Permissions::from_mode(0o755))?;
            file.as_file().sync_all()?;
            file.persist_noclobber(path).map_err(|error| error.error)?;
        } else if !enabled {
            Self::remove(path)?;
        }
        if enabled {
            // A prior save may have been interrupted after creating init.d or publishing the hook.
            fs::File::open(parent.parent().context("boot hook directory parent")?)?.sync_all()?;
        }
        Ok(fs::File::open(parent)?.sync_all()?)
    }
    fn remove(path: &Path) -> Result<()> {
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
    #[cfg(not(miri))]
    fn extended_preferences_never_overwrite_the_pre_feature_snapshot() {
        let dir = crate::application::tests::sandbox();
        let files = Files::sandbox(dir.path()).unwrap();
        let legacy = Config {
            instance_url: Some("https://old.example".parse().unwrap()),
            ..Config::default()
        };
        let mut extended = Config {
            location: ProcessingLocation::Embedded,
            ..Config::default()
        };
        files.persist(&extended).unwrap();
        let path = dir.path().join("config.json");
        assert!(!path.exists(), "legacy install boots unconfigured");
        let saved = files.load().unwrap();
        assert_eq!(
            saved.location,
            if cfg!(feature = "embedded-prysm") {
                ProcessingLocation::Embedded
            } else {
                ProcessingLocation::Remote
            }
        );
        let snapshot = serde_json::to_vec_pretty(&legacy).unwrap();
        fs::write(&path, &snapshot).unwrap();
        extended.instance_url = Some("https://new.example".parse().unwrap());
        files.persist(&extended).unwrap();
        assert_eq!(files.load().unwrap().instance_url, extended.instance_url);
        assert_eq!(fs::read(path).unwrap(), snapshot);
    }

    #[test]
    #[cfg(not(miri))]
    fn migrate_legacy_output_once_without_replacing_even_an_invalid_portal_store() {
        let dir = crate::application::tests::sandbox();
        let files = Files::sandbox(dir.path()).unwrap();
        let mut saved = serde_json::to_value(Config {
            location: ProcessingLocation::Embedded,
            ..Config::default()
        })
        .unwrap();
        let output = serde_json::json!({
            "type":"wled","url":"http://lights.example/",
            "leds":{"top":1,"right":2,"bottom":3,"left":4}
        });
        saved["embeddedOutput"] = output.clone();
        let old_path = dir.path().join("config-v2.json");
        let old_bytes = serde_json::to_vec(&saved).unwrap();
        fs::write(&old_path, &old_bytes).unwrap();
        let prefs = files.load().unwrap();
        let path = files.portal_settings_path();
        let settings: prysm_contract::Settings =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        settings.validate().unwrap();
        assert_eq!(serde_json::to_value(&settings.output).unwrap(), output);
        assert!(matches!(
            settings.input,
            Some(prysm_contract::Input::Webos { .. })
        ));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        // Replaying a half-complete migration must not overwrite portal edits.
        let current = dir.path().join("config-v3.json");
        fs::remove_file(&current).unwrap();
        fs::write(&path, b"damaged existing store").unwrap();
        files.load().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"damaged existing store");
        // A deliberately removed portal store is not seeded a second time.
        fs::remove_file(&path).unwrap();
        files.persist(&prefs).unwrap();
        files.load().unwrap();
        assert!(!path.exists());
        assert_eq!(fs::read(old_path).unwrap(), old_bytes);
        assert!(
            !fs::read_to_string(current)
                .unwrap()
                .contains("embeddedOutput")
        );
    }
    #[test]
    #[cfg(not(miri))]
    fn cleanup_rejects_a_boot_hook_pipe_without_waiting_for_a_writer() {
        use nix::{sys::stat::Mode, unistd::mkfifo};
        let dir = crate::application::tests::sandbox();
        let files = Files::sandbox(dir.path()).unwrap();
        mkfifo(&files.hook_path, Mode::S_IRUSR).unwrap();
        assert!(files.autostart(false).is_err());
        assert!(files.hook_path.exists());
    }

    #[test]
    #[cfg(not(miri))]
    fn an_interruption_at_config_commit_keeps_autostart_bootable() {
        let dir = crate::application::tests::sandbox();
        let mut files = Files::sandbox(dir.path()).unwrap();
        files.hook_path = dir.path().join("init.d/prysm-capture");
        let mut config = Config {
            instance_url: Some("http://127.0.0.1:8787".parse().unwrap()),
            ..Config::default()
        };
        files.persist(&config).unwrap();
        for enable in [true, false] {
            config.autostart = enable;
            let staged = Files::stage_config(
                &dir.path().join("config.json"),
                &serde_json::to_vec(&config).unwrap(),
            )
            .unwrap();
            files
                .commit_with_autostart(enable, || {
                    // Inspect both sides of the rename before any further hook operation.
                    assert_eq!(fs::read(&files.hook_path)?, files.boot_script.as_bytes());
                    assert_eq!(files.load()?.autostart, !enable);
                    staged.commit()?;
                    assert_eq!(files.load()?.autostart, enable);
                    assert!(files.has_boot_hook()?);
                    Ok(())
                })
                .unwrap();
            assert_eq!(files.has_boot_hook().unwrap(), enable);
        }
    }

    #[test]
    #[cfg(not(miri))]
    fn rollback_after_publication_restores_the_original_bytes_or_absence() {
        let dir = crate::application::tests::sandbox();
        let files = Files::sandbox(dir.path()).unwrap();
        let path = dir.path().join("config.json");
        let previous = br#"{ "metrics": false }"#;
        fs::write(&path, b"replacement").unwrap();
        files.restore_config(&path, Some(previous)).unwrap();
        assert_eq!(fs::read(&path).unwrap(), previous);
        files.restore_config(&path, None).unwrap();
        assert!(!path.exists());
    }

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
            instance_url: Some("http://127.0.0.1:8787".parse().unwrap()),
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
