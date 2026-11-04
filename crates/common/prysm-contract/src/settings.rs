//! Application settings shared by JSON storage and the browser.
use crate::{Config, ConfigRevision, Input, Output, ProcessingSettings, Source};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub calibration: crate::CalibrationProfiles,
    pub source: Source,
    pub input: Option<Input>,
    pub output: Option<Output>,
    pub processing: ProcessingSettings,
}

impl Settings {
    pub fn validate(&self) -> anyhow::Result<()> {
        self.calibration.validate()?;
        if let Some(input) = &self.input {
            input.validate()?;
        }
        if let Some(output) = &self.output {
            output.validate()?;
        }
        Ok(())
    }

    #[must_use]
    pub fn config(&self, revision: ConfigRevision) -> Option<Config> {
        Some(Config {
            correction: self.calibration.correction(),
            revision,
            source: self.source,
            input: self.input.clone()?,
            output: self.output.clone()?,
            processing: self.processing,
        })
    }

    #[must_use]
    pub fn missing_required(&self) -> Vec<RequiredSetting> {
        let mut missing = Vec::new();
        if self.input.is_none() {
            missing.push(RequiredSetting::Input);
        }
        if self.output.is_none() {
            missing.push(RequiredSetting::Output);
        }
        missing
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RequiredSetting {
    Input,
    Output,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SettingsView {
    pub revision: ConfigRevision,
    pub settings: Settings,
    pub missing_required: Vec<RequiredSetting>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateSettings {
    pub expected_revision: ConfigRevision,
    pub settings: Settings,
}
