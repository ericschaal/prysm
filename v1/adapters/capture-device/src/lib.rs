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

use prysm_ports::{Candidate, FrameSource, Result, SourceFactory, ValidationError};
use serde::Serialize;

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

    /// The video devices present (V4L2 nodes that capture, `AVFoundation`
    /// devices), each filling `path`. Listing starts no capture.
    fn candidates(&self) -> Vec<Candidate> {
        #[derive(Serialize)]
        struct Path<'a> {
            path: &'a str,
        }
        prysm_capture::list_devices()
            .into_iter()
            .map(|device| Candidate::new(device.name, &Path { path: &device.id }).detail(device.id))
            .collect()
    }
}

#[cfg(test)]
mod tests;
