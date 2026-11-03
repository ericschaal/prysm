//! Shared settings fixtures for application unit tests.
use crate::settings::SettingsStore;
use prysm_contract::{Config, Settings, UpdateSettings};

pub(crate) fn config() -> Config {
    serde_json::from_str(include_str!("../tests/fixtures/config.json")).unwrap()
}

pub(crate) fn settings_value(config: &Config) -> serde_json::Value {
    let mut value = serde_json::to_value(config).unwrap();
    value.as_object_mut().unwrap().remove("revision");
    value
}

pub(crate) fn store(directory: &tempfile::TempDir, config: &Config) -> SettingsStore {
    let path = directory.path().join("settings.json");
    std::fs::write(&path, serde_json::to_vec(&settings_value(config)).unwrap()).unwrap();
    SettingsStore::load(path).unwrap()
}

pub(crate) fn update(app: &prysm_api::App, config: &Config) -> UpdateSettings {
    let settings: Settings = serde_json::from_value(settings_value(config)).unwrap();
    UpdateSettings {
        expected_revision: app.settings().revision,
        settings,
    }
}
