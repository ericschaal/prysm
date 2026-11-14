use crate::{Result, Settings, ValidationError};
use prysm_core::{EdgeColors, Frame};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::time::Duration;

/// What [`FrameSource::next_frame`] produced.
#[derive(Debug)]
pub enum Pull {
    Frame(Frame),
    /// No frame within the timeout; the caller checks its stop flag and asks again.
    Idle,
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
            Self::Idle | Self::Ended => None,
        }
    }

    #[must_use]
    pub fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }
}

/// A frame, or `Idle` without one: what a push-style source's newest-frame
/// hand-off yields.
impl From<Option<Frame>> for Pull {
    fn from(frame: Option<Frame>) -> Self {
        frame.map_or(Self::Idle, Self::Frame)
    }
}

/// A capture device or remote frame receiver.
pub trait FrameSource: Send {
    /// Wait up to `timeout` for the next frame. `Err` is a failure; a source
    /// that has nothing more to give returns [`Pull::Ended`], and keeps
    /// returning it.
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
    /// Called once after the last `write`; also the failure-compensation path.
    fn idle(self: Box<Self>, mode: IdleMode) -> Result<()>;
    /// Why the device is not showing the written colours, as far as it
    /// last said; `None` while it shows them or the output cannot tell.
    /// Called on the session thread after every `write`, so it returns
    /// within milliseconds: an output that asks its device does so at its
    /// own low rate and without waiting for the answer.
    fn observe(&mut self) -> Option<OutputIssue> {
        None
    }
    /// The device's own brightness, as far as it last said, while it scales
    /// the written colours; `None` while it does not or the output cannot
    /// tell. Learned by the same checks as [`observe`](Self::observe), read
    /// right after it and as cheap.
    fn brightness(&self) -> Option<DeviceBrightness> {
        None
    }
}

/// The device's own brightness while it scales Prysm's colours: the LEDs
/// show them at [`percent`](Self::percent) of the brightness Prysm asks for.
/// Prysm reports it and never changes it; the device's owner sets it there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "UncheckedBrightness")]
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
            // Never above 100: `level` is at most `full`.
            percent: u8::try_from(percent).unwrap_or(100),
        }
    }

    /// 0 to 100.
    #[must_use]
    pub const fn percent(self) -> u8 {
        self.percent
    }
}

/// A [`DeviceBrightness`] as it arrives, before its range is checked.
#[derive(Deserialize)]
struct UncheckedBrightness {
    percent: u8,
}

impl TryFrom<UncheckedBrightness> for DeviceBrightness {
    type Error = ValidationError;

    fn try_from(unchecked: UncheckedBrightness) -> Result<Self, ValidationError> {
        Self::from_percent(unchecked.percent)
            .ok_or_else(|| ValidationError::new("percent", "must be at most 100"))
    }
}

/// Why the LEDs are not showing Prysm's colours although lighting runs.
/// Prysm reports it and keeps sending: the LEDs follow again as soon as the
/// device takes the colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputIssue {
    /// The device was switched off.
    Off,
    /// The device's own override is on (WLED: "live override").
    Overridden,
    /// The device is not taking the colours (realtime reception disabled,
    /// or it left realtime mode).
    NotLive,
    /// Another sender's colours are reaching the device.
    OtherSender,
    /// The device does not answer state requests.
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
    /// `Ok(None)`: nothing saved yet.
    fn load(&mut self) -> Result<Option<Settings>>;
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
        let decoded: DeviceBrightness = serde_json::from_str(r#"{"percent":40}"#).unwrap();
        assert_eq!(decoded.percent(), 40);
        assert_eq!(
            serde_json::to_string(&decoded).unwrap(),
            r#"{"percent":40}"#
        );
        let error = serde_json::from_str::<DeviceBrightness>(r#"{"percent":101}"#).unwrap_err();
        assert!(error.to_string().contains("at most 100"), "{error}");
    }
}
