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

/// What a device source's stop waits for: its pulls return within their
/// timeout, then `close` releases the device.
pub const STOP: prysm_ports::SourceStop = prysm_ports::SourceStop {
    frame: std::time::Duration::ZERO,
    end: std::time::Duration::ZERO,
    close: source::Deadlines::DEVICE.release,
};

use prysm_ports::{
    Candidate, Error, FrameSource, Hangup, Mode, Modes, Readiness, Result, SourceFactory,
    ValidationError, capture_size,
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
    /// bounded wait that covers both.
    fn open(&self, settings: &DeviceSettings) -> Result<Box<dyn FrameSource>> {
        self.open_for_sync(settings, &Hangup::default())
    }

    fn open_for_sync(
        &self,
        settings: &DeviceSettings,
        stop: &Hangup,
    ) -> Result<Box<dyn FrameSource>> {
        if stop.is_cut() {
            return Err(Error::adapter(
                "capture open cancelled",
                prysm_ports::Stopping,
            ));
        }
        let check: source::Check = Box::new({
            let (factory, settings) = (*self, settings.clone());
            move || {
                let modes = factory.modes(&settings);
                Ok(modes.check(settings.width, settings.height, settings.fps)?)
            }
        });
        let fps = NonZeroU32::new(settings.fps)
            .ok_or_else(|| ValidationError::new("fps", "must be a positive frame rate"))?;
        let (device, width, height) = (settings.device.clone(), settings.width, settings.height);
        let capture: source::Capture = Box::new(move |sink| {
            prysm_capture_device_sys::capture(Some(&device), width, height, fps, sink)
        });
        Ok(Box::new(source::DeviceFrames::start_with_stop(
            check,
            capture,
            source::Deadlines::DEVICE,
            stop,
        )?))
    }

    fn check(&self, settings: &DeviceSettings, _stop: &Hangup) -> Readiness<Error> {
        match prysm_capture_device_sys::check_device(&settings.device) {
            Ok(()) => {
                match self
                    .modes(settings)
                    .check(settings.width, settings.height, settings.fps)
                {
                    Ok(()) => Readiness::Ready,
                    Err(error) => Readiness::Blocked(error.into()),
                }
            }
            Err(error) => self.failure(source::failure("cannot use the video device", error)),
        }
    }

    fn failure(&self, error: Error) -> Readiness<Error> {
        use prysm_capture_device_sys::CaptureError;
        if matches!(
            error.cause::<CaptureError>(),
            Some(CaptureError::DeviceNotFound { .. } | CaptureError::Disconnected { .. })
        ) {
            return Readiness::Waiting(error);
        }
        Readiness::Blocked(error)
    }

    fn watch(&self, settings: &DeviceSettings, stop: &Hangup) -> Option<Readiness<Error>> {
        while !stop.is_cut() {
            match prysm_capture_device_sys::watch_device(&settings.device, stop) {
                Ok(false) => return None,
                Ok(true) => {
                    let ready = self.check(settings, stop);
                    if !matches!(ready, Readiness::Waiting(_)) {
                        return (!stop.is_cut()).then_some(ready);
                    }
                }
                Err(error) => {
                    return Some(Readiness::Blocked(Error::adapter(
                        "cannot watch for video device",
                        error,
                    )));
                }
            }
        }
        None
    }

    /// The video devices present (V4L2 nodes that capture, `AVFoundation`
    /// devices), each filling `device`. Listing starts no capture.
    fn candidates(&self) -> Vec<Candidate> {
        #[derive(Serialize)]
        struct Fields<'a> {
            device: &'a str,
        }
        prysm_capture_device_sys::list_devices()
            .into_iter()
            .map(|device| {
                Candidate::new(device.name, &Fields { device: &device.id }).detail(device.id)
            })
            .collect()
    }

    /// The `device`'s formats, within what [`DeviceSettings::validate`]
    /// accepts; unknown when the device cannot be opened or asked.
    fn modes(&self, settings: &DeviceSettings) -> Modes {
        prysm_capture_device_sys::list_formats(&settings.device).map_or(Modes::Unknown, |listed| {
            Modes::Known {
                modes: bounded(&listed),
            }
        })
    }
}

/// `listed` within what [`DeviceSettings::validate`] accepts
/// ([`capture_size::SIZES`]), sorted and without repeats.
fn bounded(listed: &[Mode]) -> Vec<Mode> {
    listed
        .iter()
        .filter_map(|mode| mode.and(&capture_size::SIZES))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod mode_tests {
    use super::*;
    use prysm_ports::Span;

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
            Mode::new(exactly(1280), exactly(720), span(1, 240, 1)),
            Mode::new(exactly(1280), exactly(720), exactly(240)),
        ]);
        assert_eq!(
            modes,
            [
                Mode::new(span(2, 3840, 2), Span::exactly(480), Span::exactly(60)),
                Mode::new(Span::exactly(640), Span::exactly(360), span(1, 30, 1)),
                Mode::new(Span::exactly(1280), Span::exactly(720), span(1, 120, 1)),
                Mode::new(Span::exactly(1920), Span::exactly(1080), span(1, 30, 1)),
            ]
        );
    }
}
