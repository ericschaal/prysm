//! Capture, processing, rendering, configuration ownership, and REST control.
mod application;
pub use application::run;

pub mod config;
pub mod control;
mod input;
pub mod video;

pub mod actors;

/// Validate saved settings against this executable's enabled features.
pub fn validate_config(config: &prysm_contract::Config) -> anyhow::Result<()> {
    config.validate()?;
    anyhow::ensure!(
        cfg!(feature = "desktop") || !config.output.is_egui(),
        "egui output requires building with --features desktop"
    );
    Ok(())
}
