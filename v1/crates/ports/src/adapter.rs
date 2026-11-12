use crate::{Result, Settings};
use prysm_core::{EdgeColors, Frame};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// A capture device or remote frame receiver.
pub trait FrameSource: Send {
    /// `Ok(None)`: no frame within `timeout`; the caller then checks its stop flag.
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<Frame>>;
    /// Release native resources. Returning `Ok` certifies release.
    fn close(self: Box<Self>) -> Result<()>;
}

/// A boxed source is a source, so `Registry::open_source` results can be
/// handed to anything generic over `FrameSource`.
impl<T: FrameSource + ?Sized> FrameSource for Box<T> {
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<Frame>> {
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
}

impl<T: FrameSink + ?Sized> FrameSink for Box<T> {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        (**self).send(frame)
    }
}

/// Persisted settings. Blocking; the engine calls it off the async runtime.
pub trait SettingsStore: Send {
    /// `Ok(None)`: nothing saved yet.
    fn load(&mut self) -> Result<Option<Settings>>;
    fn save(&mut self, settings: &Settings) -> Result<()>;
}
