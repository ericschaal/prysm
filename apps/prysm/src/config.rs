//! HTTP deployment policy and the application subset owned by a capture pipeline.
use prysm_contract::{Config, ConfigRevision, Input, Output, ProcessingSettings};
use std::net::SocketAddr;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebConfig {
    pub listen: SocketAddr,
    pub allowed_hosts: Vec<String>,
}
impl WebConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.allowed_hosts.is_empty()
                && self.allowed_hosts.iter().all(|host| {
                    host == "*"
                        || (host.trim() == host
                            && !host.is_empty()
                            && !host.contains(['/', '@', '?', '#'])
                            && !host.chars().any(char::is_whitespace))
                }),
            "configure at least one valid allowed panel host"
        );
        Ok(())
    }
}

/// The configuration needed to create one pipeline session.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineConfig {
    pub correction: prysm_contract::ColorCorrection,
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
            correction: config.correction,
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
        let config: Config = crate::test_support::config();
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
        };
        assert!(current.requires_restart(&next));
        let mut next = current.clone();
        let Output::Wled { url, .. } = &mut next.output;
        {
            *url = "http://changed-wled.local/".parse().unwrap();
        }
        assert!(current.requires_restart(&next));
    }
}
