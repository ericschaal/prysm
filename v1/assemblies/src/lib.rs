//! Host-side types for reviewing composition. Examples use explicitly fake adapters.
pub use prysm_capture_desktop::DesktopCaptureSettings;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerExit {
    WebPortal,
    Preview,
    Capture,
    Processing,
    Output,
    Coordinator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostAction {
    ReportOnly,
    NotifyCoordinator,
    EmergencyCleanup,
}

/// The host may replace this function without changing any port or data connection.
pub fn v1_host_action(exit: OwnerExit) -> HostAction {
    match exit {
        OwnerExit::WebPortal | OwnerExit::Preview => HostAction::ReportOnly,
        OwnerExit::Capture | OwnerExit::Processing | OwnerExit::Output => {
            HostAction::NotifyCoordinator
        }
        OwnerExit::Coordinator => HostAction::EmergencyCleanup,
    }
}

pub mod tv;

pub mod server;
pub use prysm_remote_capture::RemoteCaptureSettings;
pub use server::ServerCaptureSettings;
