//! Concrete headless/TUI runtimes and host-owned settings. Web/TV examples use fixtures.
pub use prysm_capture_device::DeviceCaptureSettings;
pub use prysm_capture_webos::TvCaptureSettings;
use prysm_contract::{
    capture::CaptureControl, output::OutputControl, processing::ProcessingControl,
};
pub use prysm_output_wled::WledSettings;
#[cfg(feature = "schema")]
pub mod contracts;

/// Retained by the host before the regular controls move into the coordinator.
pub struct EmergencyControls<C: CaptureControl, P: ProcessingControl, O: OutputControl> {
    pub capture: C,
    pub processing: P,
    pub output: O,
}

pub mod tv;

mod server_capture;
pub use prysm_remote_capture::RemoteCaptureSettings;
pub use server_capture::ServerCaptureSettings;

#[cfg(any(feature = "headless", feature = "tui", test))]
mod host;
#[cfg(any(feature = "headless", feature = "tui", test))]
pub use host::{HostError, HostEvent};
#[cfg(feature = "headless")]
pub mod headless;
#[cfg(feature = "tui")]
pub mod tui;
