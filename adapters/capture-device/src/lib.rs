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

use prysm_ports::{
    Candidate, FrameSource, Mode, Modes, Result, SourceFactory, Span, ValidationError, capture_size,
};
use serde::Serialize;
use std::collections::BTreeSet;
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

    /// Checks the size and rate against [`Self::modes`] (when the device
    /// answers), then blocks until the device is capturing, or fails within a
    /// bounded wait.
    fn open(&self, settings: &DeviceSettings) -> Result<Box<dyn FrameSource>> {
        self.modes(settings)
            .check(settings.width, settings.height, settings.fps)?;
        let fps = NonZeroU32::new(settings.fps)
            .ok_or_else(|| ValidationError::new("fps", "must be a positive frame rate"))?;
        let (path, width, height) = (settings.path.clone(), settings.width, settings.height);
        let capture: source::Capture = Box::new(move |sink| {
            prysm_capture_device_sys::capture(Some(&path), width, height, fps, sink)
        });
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
        prysm_capture_device_sys::list_devices()
            .into_iter()
            .map(|device| Candidate::new(device.name, &Path { path: &device.id }).detail(device.id))
            .collect()
    }

    /// The device at `path`'s formats, within what [`DeviceSettings::validate`]
    /// accepts; unknown when the device cannot be opened or asked.
    fn modes(&self, settings: &DeviceSettings) -> Modes {
        prysm_capture_device_sys::list_formats(&settings.path)
            .map_or(Modes::Unknown, |listed| Modes::Known(bounded(&listed)))
    }
}

/// `listed` within what [`DeviceSettings::validate`] accepts
/// ([`capture_size::SIZES`], and on macOS `AVFoundation`'s `i32` rates),
/// sorted and without repeats.
fn bounded(listed: &[Mode]) -> Vec<Mode> {
    let bound = Mode::new(
        capture_size::SIZES.width(),
        capture_size::SIZES.height(),
        Span::new(1, i32::MAX.unsigned_abs(), 1).expect("a span"),
    );
    listed
        .iter()
        .filter_map(|mode| mode.and(&bound))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod mode_tests {
    use super::*;

    #[test]
    fn listed_modes_are_bounded_sorted_and_unique() {
        let exactly = Span::exactly;
        let span = |min, max, step| Span::new(min, max, step).unwrap();
        let thirty = span(1, 30, 1);
        let modes = bounded(&[
            Mode::new(exactly(1920), exactly(1080), thirty),
            Mode::new(exactly(640), exactly(360), thirty),
            Mode::new(exactly(640), exactly(360), thirty),
            Mode::new(exactly(7680), exactly(4320), thirty),
            Mode::new(span(2, 8000, 2), exactly(480), exactly(60)),
        ]);
        assert_eq!(
            modes,
            [
                Mode::new(span(2, 3840, 2), Span::exactly(480), Span::exactly(60)),
                Mode::new(Span::exactly(640), Span::exactly(360), span(1, 30, 1)),
                Mode::new(Span::exactly(1920), Span::exactly(1080), span(1, 30, 1)),
            ]
        );
    }
}
