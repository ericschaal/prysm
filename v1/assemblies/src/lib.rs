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

pub mod server;
pub use prysm_remote_capture::RemoteCaptureSettings;
pub use server::ServerCaptureSettings;
