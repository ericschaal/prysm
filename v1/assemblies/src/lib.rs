//! Concrete headless/TUI runtimes and host-owned settings. Web/TV examples use fixtures.
pub use prysm_capture_device::DeviceCaptureSettings;
pub use prysm_capture_webos::TvCaptureSettings;
pub use prysm_output_wled::WledSettings;
#[cfg(feature = "schema")]
pub mod contracts;

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
