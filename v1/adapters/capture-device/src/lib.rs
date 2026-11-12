//! Local video device capture (V4L2 on Linux, AVFoundation on macOS) as a
//! blocking [`FrameSource`].
//!
//! - [`DeviceSource`]: the factory registered under kind `"device"`.
//! - [`DeviceSettings`]: its human-editable settings and typed validation.
//! - `source`: the opened device; bridges the native worker's channels to
//!   blocking `next_frame`/`close` calls.
mod settings;
mod source;

pub use settings::DeviceSettings;

use prysm_ports::{FrameSource, Result, SourceFactory, ValidationError};

/// Opens local video devices.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeviceSource;

impl SourceFactory for DeviceSource {
    const KIND: &'static str = "device";
    type Settings = DeviceSettings;

    fn validate(&self, settings: &DeviceSettings) -> Result<(), ValidationError> {
        settings.validate()
    }

    /// Blocks until the device is capturing, or fails within a bounded wait.
    fn open(&self, settings: &DeviceSettings) -> Result<Box<dyn FrameSource>> {
        let native = source::Native::open(settings)?;
        Ok(Box::new(source::DeviceFrames::start(
            native,
            source::Deadlines::DEVICE,
        )?))
    }
}

#[cfg(test)]
mod tests;
