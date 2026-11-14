//! Capture interfaces, latest-frame delivery, and platform adapters.
pub use prysm_core::Frame;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use prysm_core::{FramePool, PixelFormat, YuvRange, YuvStandardMatrix};
mod device;
pub use device::{CaptureSink, DeviceInfo, capture, list_devices};

/// Terminal capture failures. Cancellation closes the stream successfully.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CaptureError {
    #[error("no video capture device found (requested: {device:?})")]
    DeviceNotFound {
        device: Option<String>,
        #[source]
        source: Option<std::io::Error>,
    },
    #[error("capture device disconnected: {details}")]
    Disconnected { details: String },
    #[error("unsupported capture format: {details}")]
    UnsupportedFormat {
        details: String,
        #[source]
        source: Option<std::io::Error>,
    },
    #[error("native capture cleanup failed: {0}")]
    Cleanup(#[source] anyhow::Error),
    #[error(transparent)]
    Backend(#[from] anyhow::Error),
}

type Result<T> = std::result::Result<T, CaptureError>;

#[cfg(any(target_os = "linux", test))]
fn check_format_rejection(error: std::io::Error) -> Result<std::io::Error> {
    if error.kind() == std::io::ErrorKind::InvalidInput {
        Ok(error)
    } else {
        Err(anyhow::Error::new(error)
            .context("failed to set capture format")
            .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_negotiation_only_falls_back_for_invalid_input() {
        use std::io::{Error, ErrorKind};
        assert_eq!(
            check_format_rejection(Error::from(ErrorKind::InvalidInput))
                .unwrap()
                .kind(),
            ErrorKind::InvalidInput,
        );
        for kind in [
            ErrorKind::ResourceBusy,
            ErrorKind::PermissionDenied,
            ErrorKind::Other,
        ] {
            let error = check_format_rejection(Error::from(kind)).unwrap_err();
            let CaptureError::Backend(error) = error else {
                panic!("expected backend error for {kind:?}")
            };
            assert_eq!(error.downcast_ref::<Error>().unwrap().kind(), kind);
            assert!(error.to_string().contains("failed to set capture format"));
        }
    }

    #[test]
    fn capture_errors_preserve_causes_at_the_application_boundary() {
        let error = CaptureError::UnsupportedFormat {
            details: "test format".into(),
            source: Some(std::io::Error::from(std::io::ErrorKind::InvalidInput)),
        };
        let error = anyhow::Error::new(error).context("starting camera");
        assert!(matches!(
            error.downcast_ref::<CaptureError>(),
            Some(CaptureError::UnsupportedFormat { .. })
        ));
        assert_eq!(
            error
                .chain()
                .find_map(|cause| cause.downcast_ref::<std::io::Error>())
                .unwrap()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
        let backend = CaptureError::Backend(
            anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
                .context("opening camera"),
        );
        let backend = anyhow::Error::new(backend).context("starting camera");
        assert_eq!(
            backend
                .chain()
                .find_map(|cause| cause.downcast_ref::<std::io::Error>())
                .unwrap()
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
mod capture_time;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(target_os = "linux", target_os = "macos", test))]
mod trace_stats;

#[cfg(target_os = "macos")]
mod macos;
