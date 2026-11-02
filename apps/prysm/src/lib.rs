//! Capture, processing, rendering, configuration ownership, and REST control.
mod application;
pub use application::run;

pub mod config;
pub mod control;
mod input;
mod preview;
pub mod video;

pub mod pipeline;

/// Validate saved settings before starting the application or saving a mutation.
pub fn validate_config(config: &prysm_contract::Config) -> anyhow::Result<()> {
    config.validate()
}
