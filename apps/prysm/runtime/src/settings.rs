//! Validate and atomically save application settings before publishing them.
use anyhow::{Context as _, ensure};
use atomic_write_file::AtomicWriteFile;
use prysm_contract::{
    CalibrationProfile, Config, ConfigRevision, Settings, SettingsView, Source, UpdateSettings,
};
use std::{
    io::Write as _,
    path::{Path, PathBuf},
};

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("{0}")]
    Conflict(&'static str),
    #[error("invalid settings: {0:#}")]
    Invalid(#[source] anyhow::Error),
    #[error("settings storage failed: {0:#}")]
    Persistence(#[source] anyhow::Error),
}

pub struct SettingsStore {
    path: PathBuf,
    exists: bool,
    view: SettingsView,
}

fn validate(settings: &Settings, native_webos: bool) -> Result<(), SettingsError> {
    if native_webos && !matches!(settings.input, Some(prysm_contract::Input::Webos { .. })) {
        return Err(SettingsError::Invalid(anyhow::anyhow!(
            "the TV portal requires native webOS capture"
        )));
    }
    settings.validate().map_err(SettingsError::Invalid)
}

impl SettingsStore {
    pub fn load(path: PathBuf) -> Result<Self, SettingsError> {
        Self::open(path, false)
    }

    pub fn load_native(path: PathBuf) -> Result<Self, SettingsError> {
        Self::open(path, true)
    }

    fn open(path: PathBuf, native_webos: bool) -> Result<Self, SettingsError> {
        let (mut settings, exists): (Settings, _) = match std::fs::read(&path) {
            Ok(bytes) => (
                serde_json::from_slice(&bytes)
                    .with_context(|| format!("cannot parse settings {}", path.display()))
                    .map_err(SettingsError::Invalid)?,
                true,
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (Settings::default(), false)
            }
            Err(error) => {
                return Err(SettingsError::Persistence(
                    anyhow::Error::new(error)
                        .context(format!("cannot read settings {}", path.display())),
                ));
            }
        };
        if native_webos && settings.input.is_none() {
            settings.input = Some(prysm_contract::Input::Webos {
                refresh_rate_hz: 30.try_into().expect("valid rate"),
                capture_resolution: [1280, 720].try_into().expect("valid resolution"),
            });
        }
        validate(&settings, native_webos)?;
        Ok(Self {
            path,
            exists,
            view: SettingsView {
                native_webos,
                revision: ConfigRevision::new_session(),
                missing_required: settings.missing_required(),
                settings,
            },
        })
    }

    #[must_use]
    pub fn view(&self) -> SettingsView {
        self.view.clone()
    }

    #[must_use]
    pub fn config(&self) -> Option<Config> {
        self.view.settings.config(self.view.revision)
    }

    pub(crate) async fn set_source(
        &mut self,
        source: Source,
    ) -> Result<Option<Config>, SettingsError> {
        if self.config().is_none() {
            return Err(SettingsError::Invalid(anyhow::anyhow!(
                "complete settings before selecting a lighting source"
            )));
        }
        if self.view.settings.source == source {
            return Ok(None);
        }
        let mut settings = self.view.settings.clone();
        settings.source = source;
        self.update(UpdateSettings {
            expected_revision: self.view.revision,
            settings,
        })
        .await?;
        Ok(self.config())
    }

    pub(crate) async fn save_profile(
        &mut self,
        profile: &CalibrationProfile,
    ) -> Result<(), SettingsError> {
        let mut settings = self.view.settings.clone();
        let profiles = &mut settings.calibration.profiles;
        if let Some(existing) = profiles.iter_mut().find(|p| p.id == profile.id) {
            existing.clone_from(profile);
        } else {
            profiles.push(profile.clone());
        }
        settings.calibration.active = Some(profile.id);
        self.update(UpdateSettings {
            expected_revision: self.view.revision,
            settings,
        })
        .await?;
        Ok(())
    }

    pub(crate) async fn update(
        &mut self,
        update: UpdateSettings,
    ) -> Result<SettingsView, SettingsError> {
        if update.expected_revision != self.view.revision {
            return Err(SettingsError::Conflict(
                "settings changed; reload before saving",
            ));
        }
        validate(&update.settings, self.view.native_webos)?;
        let revision = self
            .view
            .revision
            .next()
            .map_err(|error| SettingsError::Invalid(anyhow::anyhow!(error)))?;
        let path = self.path.clone();
        let initial = !self.exists;
        let settings = tokio::task::spawn_blocking(move || {
            let bytes = serde_json::to_vec_pretty(&update.settings)
                .map_err(|error| SettingsError::Persistence(error.into()))?;
            write(&path, &bytes, initial).map_err(SettingsError::Persistence)?;
            Ok::<_, SettingsError>(update.settings)
        })
        .await
        .map_err(|error| SettingsError::Persistence(error.into()))??;
        self.exists = true;
        self.view = SettingsView {
            native_webos: self.view.native_webos,
            revision,
            missing_required: settings.missing_required(),
            settings,
        };
        Ok(self.view())
    }
}

fn write(path: &Path, bytes: &[u8], initial: bool) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    ensure!(
        parent.is_dir(),
        "settings directory {} does not exist",
        parent.display()
    );
    if initial {
        let mut file = tempfile::NamedTempFile::new_in(parent)
            .context("cannot create settings file; check directory permissions")?;
        file.write_all(bytes)?;
        file.as_file().sync_all()?;
        file.persist_noclobber(path).map_err(|error| error.error)?;
    } else {
        let mut file = AtomicWriteFile::open(path)
            .context("cannot prepare settings write; check directory permissions")?;
        file.write_all(bytes)?;
        file.commit().context("cannot replace settings")?;
    }
    #[cfg(unix)]
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn complete_document_round_trips_and_stale_writes_preserve_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let mut store = SettingsStore::load(path.clone()).unwrap();
        assert!(!path.exists());
        let initial = store.view();
        let mut settings = initial.settings;
        settings.processing.brightness_percent = 42.try_into().unwrap();
        let update = UpdateSettings {
            expected_revision: initial.revision,
            settings,
        };
        let saved = store.update(update.clone()).await.unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            serde_json::from_slice::<Settings>(&bytes).unwrap(),
            saved.settings
        );
        assert!(matches!(
            store.update(update).await,
            Err(SettingsError::Conflict(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let restored = SettingsStore::load(path).unwrap().view();
        assert_eq!(restored.settings, saved.settings);
        assert_ne!(restored.revision, saved.revision);
    }

    #[tokio::test]
    async fn failed_write_preserves_memory_and_an_externally_created_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let mut store = SettingsStore::load(path.clone()).unwrap();
        let initial = store.view();
        std::fs::write(&path, "external file").unwrap();
        let mut settings = initial.settings.clone();
        settings.source = Source::Wled;
        assert!(matches!(
            store
                .update(UpdateSettings {
                    expected_revision: initial.revision,
                    settings
                })
                .await,
            Err(SettingsError::Persistence(_))
        ));
        assert_eq!(store.view().revision, initial.revision);
        assert_eq!(store.view().settings, initial.settings);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external file");
        assert!(matches!(
            SettingsStore::load(path),
            Err(SettingsError::Invalid(_))
        ));
    }

    #[test]
    fn persisted_host_input_is_rejected_on_native_startup() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let settings = Settings {
            input: Some(crate::test_support::config().input),
            ..Settings::default()
        };
        std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
        assert!(matches!(
            SettingsStore::load_native(path),
            Err(SettingsError::Invalid(_))
        ));
    }

    #[test]
    fn deployment_fields_are_not_application_settings() {
        let mut settings = serde_json::to_value(Settings::default()).unwrap();
        settings["web"] = serde_json::json!({"listen":"0.0.0.0:8787"});
        assert!(serde_json::from_value::<Settings>(settings).is_err());
        assert!(serde_json::from_value::<prysm_contract::Input>(serde_json::json!({
            "type":"webos", "listen":"0.0.0.0:19400", "capture_resolution":[640,360], "refresh_rate_hz":30
        })).is_err());
    }
}
