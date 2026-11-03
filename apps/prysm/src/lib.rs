//! Capture, processing, rendering, configuration ownership, and REST control.
mod application;
pub use application::run;
mod settings;
pub use settings::{SettingsError, SettingsStore};

mod config;
pub use config::WebConfig;
mod control;
mod input;
mod preview;
pub mod video;

mod pipeline;

#[cfg(test)]
mod test_support;
