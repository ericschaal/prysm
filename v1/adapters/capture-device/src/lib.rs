//! Local video device capture (V4L2 on Linux, AVFoundation on macOS) as a
//! blocking [`FrameSource`].
//!
//! - [`DeviceSource`]: the factory registered under kind `"device"`.
//! - [`DeviceSettings`]: its human-editable settings and typed validation.
//! - `source`: the opened device; an owner thread runs the blocking native
//!   capture behind `next_frame`/`close`.
mod settings;
mod source;

pub use settings::DeviceSettings;

use prysm_ports::{Candidate, FrameSource, Result, SourceFactory, ValidationError};
use serde::Serialize;
use std::num::NonZeroU32;

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
        let fps = NonZeroU32::new(settings.fps)
            .ok_or_else(|| ValidationError::new("fps", "must be a positive frame rate"))?;
        let (path, width, height) = (settings.path.clone(), settings.width, settings.height);
        let capture: source::Capture =
            Box::new(move |sink| prysm_capture::capture(Some(&path), width, height, fps, sink));
        Ok(Box::new(source::DeviceFrames::start(
            capture,
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
