use crate::{Result, Settings};
use prysm_core::{EdgeColors, Frame};
use serde::{Deserialize, Serialize};
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

/// A frame, or `Idle` without one: what a push-style source's
/// [`LatestSlot`](crate::LatestSlot) yields.
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
    fn idle(&mut self) -> Result<()> {
        Ok(())
    }
}

impl<T: FrameSink + ?Sized> FrameSink for Box<T> {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        (**self).send(frame)
    }
    fn idle(&mut self) -> Result<()> {
        (**self).idle()
    }
}

/// Persisted settings. Blocking; the engine calls it off the async runtime.
pub trait SettingsStore: Send {
    /// `Ok(None)`: nothing saved yet.
    fn load(&mut self) -> Result<Option<Settings>>;
    fn save(&mut self, settings: &Settings) -> Result<()>;
}
