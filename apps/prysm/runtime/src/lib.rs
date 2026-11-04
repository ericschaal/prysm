//! Capture, processing, rendering, configuration ownership, and REST control.
mod application;
#[cfg(feature = "host-input")]
pub use application::run;
pub use application::run_native;
pub use input::{InputStream, NativeCapture};
mod settings;
pub use settings::{SettingsError, SettingsStore};

mod config;
pub use config::WebConfig;
mod control;
mod input;
mod preview;
#[cfg(feature = "host-input")]
pub mod video;

mod pipeline;

#[cfg(test)]
mod test_support;
