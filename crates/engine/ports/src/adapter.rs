use crate::{
    DeviceReport, Hangup, OutputObservationState, OutputRelease, Result, Settings, ValidationError,
};
use prysm_core::{EdgeColors, Frame};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::time::Duration;

/// What [`FrameSource::next_frame`] produced.
#[derive(Debug)]
pub enum Pull {
    Frame(Frame),
    /// No frame within the timeout, and no reason given (no signal, no
    /// sender); the caller checks its stop flag and asks again. Not a failure.
    Quiet,
    /// No frame within the timeout because the source's device refuses to
    /// share the video it shows; frames come again once it shares it. The
    /// caller asks again, as after [`Pull::Quiet`]. Not a failure.
    Refused,
    /// The source has finished (a file played to its end) and will produce no
    /// more frames. Not a failure: the session stops normally.
    Ended,
}

impl Pull {
    /// The frame, if this is one.
    #[must_use]
    pub fn into_frame(self) -> Option<Frame> {
        match self {
            Self::Frame(frame) => Some(frame),
            Self::Quiet | Self::Refused | Self::Ended => None,
        }
    }
}

/// How long a source can keep a session's stop waiting, beyond the timeout
/// the pull in flight was given: what a host's stop deadline must cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceStop {
    /// A pull in flight that returns a frame, after its timeout (the rest
    /// of a frame that began); the frame is then written before the loop
    /// sees the stop (an open it needs ends at once: see
    /// [`OutputFactory::open_for_sync`](crate::OutputFactory::open_for_sync)).
    pub frame: Duration,
    /// A pull in flight that ends the source (its end or a failure), after
    /// its timeout; nothing is opened after it.
    pub end: Duration,
    /// `close`, releasing what the source holds.
    pub close: Duration,
}

/// A capture device or remote frame receiver.
pub trait FrameSource: Send {
    /// Wait up to `timeout` for the next frame. `Err` is a failure; a source
    /// that has nothing more to give returns [`Pull::Ended`], and keeps
    /// returning it.
    ///
    /// The frame returned is the newest one: a frame that arrives before the
    /// last was taken replaces it, never queues behind it (see
    /// [`OwnerThread`](crate::OwnerThread)).
    fn next_frame(&mut self, timeout: Duration) -> Result<Pull>;
    /// Release native resources. Returning `Ok` certifies release.
    fn close(self: Box<Self>) -> Result<()>;
}

/// A boxed source is a source, so `Registry::open_source` results can be
/// handed to anything generic over `FrameSource`.
impl<T: FrameSource + ?Sized> FrameSource for Box<T> {
    fn next_frame(&mut self, timeout: Duration) -> Result<Pull> {
        (**self).next_frame(timeout)
    }
    fn close(self: Box<Self>) -> Result<()> {
        (*self).close()
    }
}

/// An LED driver.
pub trait LedOutput: Send {
    fn write(&mut self, colors: &EdgeColors) -> Result<()>;
    /// Consume the handle. With a handoff the device keeps its own display.
    /// Remote command failure leaves physical state unconfirmed. Return
    /// [`crate::Error::Faulted`] when local release cannot be certified.
    fn release(self: Box<Self>, _release: OutputRelease, _stop: &Hangup) -> Result<()> {
        Ok(())
    }

    /// Most recent device facts, updated by the adapter's active report.
    fn stream_observation(&self) -> OutputObservationState {
        OutputObservationState::Unsupported
    }

    /// What the device says about itself, as far as it last said (see
    /// [`DeviceReport`]); empty while the output cannot tell. Called on the
    /// session thread after every `write`, so it returns within
    /// milliseconds: an output that asks its device does so at its own low
    /// rate and without waiting for the answer.
    fn report(&mut self) -> DeviceReport {
        DeviceReport::default()
    }
}

/// The device's own brightness while it scales Prysm's colours: the LEDs
/// show them at [`percent`](Self::percent) of the brightness Prysm asks for.
/// Prysm reports it and never changes it; the device's owner sets it there.
/// Serialized as the percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct DeviceBrightness {
    percent: u8,
}

impl DeviceBrightness {
    /// `percent` of the brightness Prysm asks for; `None` above 100.
    #[must_use]
    pub fn from_percent(percent: u8) -> Option<Self> {
        (percent <= 100).then_some(Self { percent })
    }

    /// `level` of `full` (WLED: 0–255), rounded to a whole percent; a level
    /// above `full` is full, and a lit device never reads 0 %.
    #[must_use]
    pub fn of(level: u32, full: NonZeroU32) -> Self {
        let full = u64::from(full.get());
        let level = u64::from(level).min(full);
        let percent = ((level * 100 + full / 2) / full).max(u64::from(level > 0));
        Self {
            percent: u8::try_from(percent).unwrap_or(100),
        }
    }

    /// 0 to 100.
    #[must_use]
    pub const fn percent(self) -> u8 {
        self.percent
    }
}

impl TryFrom<u8> for DeviceBrightness {
    type Error = ValidationError;

    fn try_from(percent: u8) -> Result<Self, ValidationError> {
        Self::from_percent(percent).ok_or_else(|| {
            ValidationError::new("device brightness", "must be a percent, at most 100")
        })
    }
}

impl From<DeviceBrightness> for u8 {
    fn from(brightness: DeviceBrightness) -> Self {
        brightness.percent
    }
}

/// Why the lights do not show what was chosen. `off`: the device is
/// switched off. `overridden`: the device's own override is on (WLED: live
/// override). `not_live`: the device is not taking the colours (WLED: not in
/// live mode, or no DDP arriving). `other_sender`: another sender's colours
/// reach the device. `unreachable`: the device does not answer Prysm's state
/// checks, the request that opens it for screen sync, or, with the target
/// `off` or `effects`, the request for that mode. During screen sync Prysm
/// keeps sending the colours; it tries an unanswered request again every few
/// seconds. The lights follow once the device takes the colours or answers.
/// Open: an app reads an issue it does not know in general words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum OutputIssue {
    /// The device was switched off.
    Off,
    /// The device's own override is on.
    Overridden,
    /// The device is not taking the colours.
    NotLive,
    /// Another sender's colours reach the device.
    OtherSender,
    /// The device does not answer: its state checks, the session's open
    /// ([`Escalation::Unanswered`](crate::Escalation)) or the idle request.
    Unreachable,
}

impl std::fmt::Display for OutputIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Off => "switched off on the device",
            Self::Overridden => "live override is on",
            Self::NotLive => "not taking realtime colours",
            Self::OtherSender => "another sender is driving it",
            Self::Unreachable => "the device does not answer",
        })
    }
}

/// What an output shows once Prysm stops driving it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdleMode {
    /// LEDs dark.
    #[default]
    Off,
    /// The device's own effect (for example WLED's preset).
    Native,
}

/// Capture-only hosts forward frames instead of colors.
pub trait FrameSink: Send {
    fn send(&mut self, frame: &Frame) -> Result<()>;
    /// No frame arrived within the source's timeout. A sink that keeps a
    /// connection may use it to connect without waiting for a frame.
    fn tick(&mut self) -> Result<()> {
        Ok(())
    }
}

impl<T: FrameSink + ?Sized> FrameSink for Box<T> {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        (**self).send(frame)
    }
    fn tick(&mut self) -> Result<()> {
        (**self).tick()
    }
}

/// Persisted settings. Blocking; the engine calls it off the async runtime.
pub trait SettingsStore: Send {
    /// Optional persisted mode, independent of device power and settings revision.
    fn load_mode(&mut self) -> Result<Option<crate::LightingMode>> {
        Ok(None)
    }
    /// Stores without persistence may keep this choice for the process lifetime only.
    fn save_mode(&mut self, _mode: crate::LightingMode) -> Result<()> {
        Ok(())
    }

    /// `Ok(None)`: nothing saved yet. `Err`: the store holds something it
    /// cannot read as settings, or cannot read at all; the engine then
    /// starts unconfigured and reports this as its failure. What could not
    /// be read is kept: the next [`save`](Self::save) must not destroy it.
    fn load(&mut self) -> Result<Option<Settings>>;
    /// Store `settings` for the next [`load`](Self::load). `Err` means they
    /// were not stored, so the engine keeps the settings it had; `Ok` that
    /// they were, even if they may not yet survive a crash.
    fn save(&mut self, settings: &Settings) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_brightness_rounds_to_whole_percent_and_a_lit_device_is_never_zero() {
        let of = |level| DeviceBrightness::of(level, NonZeroU32::new(255).unwrap()).percent();
        assert_eq!(of(255), 100);
        assert_eq!(of(128), 50);
        assert_eq!(of(1), 1);
        assert_eq!(of(0), 0);
        assert_eq!(of(300), 100);
        assert_eq!(
            DeviceBrightness::of(u32::MAX, NonZeroU32::MAX).percent(),
            100
        );
    }

    #[test]
    fn device_brightness_is_at_most_full() {
        assert_eq!(
            DeviceBrightness::from_percent(100).map(DeviceBrightness::percent),
            Some(100)
        );
        assert_eq!(DeviceBrightness::from_percent(101), None);
        let decoded: DeviceBrightness = serde_json::from_str("40").unwrap();
        assert_eq!(decoded.percent(), 40);
        assert_eq!(serde_json::to_string(&decoded).unwrap(), "40");
        let error = serde_json::from_str::<DeviceBrightness>("101").unwrap_err();
        assert!(error.to_string().contains("at most 100"), "{error}");
    }
}
