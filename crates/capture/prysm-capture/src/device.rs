//! Native device capture as one blocking call. The caller owns the thread;
//! readiness and release never depend on frame consumption.
#[cfg(feature = "async")]
mod channels;

#[cfg(feature = "async")]
pub use channels::{CaptureExit, CaptureWorker, DeviceCapture};

use crate::{CaptureError, Frame, Result};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

/// Where a running [`capture`] delivers, and how it learns to stop.
/// `frame` may also be called from a native callback thread
/// (`AVFoundation`'s queue), so a sink is shared.
pub trait CaptureSink: Send + Sync {
    /// Native activation succeeded, independent of the first frame. Not
    /// called when startup fails or is stopped.
    fn ready(&self);
    /// The newest frame. A sink keeps only the latest; capture never waits on it.
    fn frame(&self, frame: Frame);
    /// Whether to stop, after waiting up to `wait` for a stop request.
    fn should_stop(&self, wait: Duration) -> bool;
}

/// A video capture device present on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// The name the device gives itself, such as `USB Video`.
    pub name: String,
    /// What [`capture`] takes: the V4L2 node on Linux, the `AVFoundation`
    /// unique ID on macOS.
    pub id: String,
}

/// The video capture devices present, without starting any: V4L2 asks each
/// `/dev/video*` node for its capabilities, `AVFoundation` lists its devices.
/// Empty on other platforms.
#[must_use]
pub fn list_devices() -> Vec<DeviceInfo> {
    #[cfg(target_os = "macos")]
    return crate::macos::list_devices();
    #[cfg(target_os = "linux")]
    return crate::linux::list_devices();
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    Vec::new()
}

/// Capture `device` (the platform default when `None`) on the calling thread
/// until `sink` asks to stop or capture fails, then release the device.
///
/// # Errors
/// [`CaptureError::Cleanup`]: release could not be certified; do not reopen
/// the device. Any other error: capture failed and the device was released.
pub fn capture(
    device: Option<&str>,
    width: u32,
    height: u32,
    rate: NonZeroU32,
    sink: &Arc<dyn CaptureSink>,
) -> Result<()> {
    prysm_core::CaptureResolution::new(width, height)
        .map_err(|error| CaptureError::Backend(anyhow::anyhow!(error)))?;
    #[cfg(target_os = "macos")]
    return crate::macos::run(device, width, height, rate, sink);
    #[cfg(target_os = "linux")]
    return crate::linux::run(device, width, height, rate, &**sink);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (device, rate, sink);
        Err(CaptureError::Backend(anyhow::anyhow!(
            "video device capture is unsupported on this platform"
        )))
    }
}

/// A sink that never stops on its own and drops what it gets.
#[cfg(test)]
pub(crate) struct Discard;

#[cfg(test)]
impl CaptureSink for Discard {
    fn ready(&self) {}
    fn frame(&self, _: Frame) {}
    fn should_stop(&self, wait: Duration) -> bool {
        std::thread::sleep(wait);
        false
    }
}
