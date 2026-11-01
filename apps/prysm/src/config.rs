//! Configuration file loading and pipeline reconfiguration policy.
use prysm_contract::{Config, ConfigRevision, Input, Output, ProcessingSettings};
use std::path::Path;

pub fn load(path: &Path) -> anyhow::Result<Config> {
    use anyhow::Context;
    let contents =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let config: Config = toml::from_str(&contents)
        .with_context(|| format!("invalid configuration {}", path.display()))?;
    config.validate()?;
    Ok(config)
}

/// The configuration needed to create one actor pipeline session.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineConfig {
    pub revision: ConfigRevision,
    pub input: Input,
    pub output: Output,
    pub processing: ProcessingSettings,
}
impl PipelineConfig {
    #[must_use]
    pub fn requires_restart(&self, next: &Self) -> bool {
        self.input != next.input || self.output != next.output
    }
}

impl From<&Config> for PipelineConfig {
    fn from(config: &Config) -> Self {
        Self {
            revision: config.revision,
            input: config.input.clone(),
            output: config.output.clone(),
            processing: config.processing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_capture_and_output_changes_require_a_pipeline_restart() {
        let config: Config = toml::from_str(include_str!("../../../prysm.toml")).unwrap();
        let current = PipelineConfig::from(&config);
        assert_eq!(current.processing, config.processing);
        let mut next = current.clone();
        next.processing.shadow_lift_percent = prysm_contract::ShadowLiftPercent::new(50).unwrap();
        next.revision = current.revision.next().unwrap();
        assert!(!current.requires_restart(&next));
        let mut rate_change = current.clone();
        if let Input::Device {
            refresh_rate_hz, ..
        } = &mut rate_change.input
        {
            *refresh_rate_hz = 15.try_into().unwrap();
        }
        assert!(current.requires_restart(&rate_change));
        if let Input::Device {
            capture_resolution, ..
        } = &mut next.input
        {
            *capture_resolution = prysm_contract::CaptureResolution::new(1280, 720).unwrap();
        }
        assert!(current.requires_restart(&next));
        let mut next = current.clone();
        next.input = Input::Webos {
            capture_resolution: [640, 360].try_into().unwrap(),
            refresh_rate_hz: 30.try_into().unwrap(),
            listen: "0.0.0.0:19400".parse().unwrap(),
        };
        assert!(current.requires_restart(&next));
        let mut next = current.clone();
        next.output = Output::Egui {};
        assert!(current.requires_restart(&next));
    }
}
