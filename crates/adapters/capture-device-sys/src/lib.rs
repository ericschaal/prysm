//! Capture interfaces, latest-frame delivery, and platform adapters.
pub use prysm_core::Frame;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use prysm_core::{FramePool, PixelFormat, YuvRange, YuvStandardMatrix};
mod device;
#[cfg(any(target_os = "linux", target_os = "macos", test))]
mod formats;
pub use device::{CaptureSink, DeviceInfo, capture, list_devices, list_formats};

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
    Disconnected {
        details: String,
        #[source]
        source: Option<std::io::Error>,
    },
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

/// The colour matrix of a picture whose source does not name one: BT.709 for
/// HD (720 lines and up), BT.601 below, as FFmpeg and browsers assume.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn unspecified_matrix(height: u32) -> prysm_core::YuvStandardMatrix {
    if height >= 720 {
        prysm_core::YuvStandardMatrix::Bt709
    } else {
        prysm_core::YuvStandardMatrix::Bt601
    }
}

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
    fn an_unnamed_matrix_is_bt709_from_720_lines() {
        use prysm_core::YuvStandardMatrix::{Bt601, Bt709};
        for (height, matrix) in [(0, Bt601), (480, Bt601), (576, Bt601), (719, Bt601)] {
            assert_eq!(unspecified_matrix(height), matrix, "{height}");
        }
        for height in [720, 1080, 2160] {
            assert_eq!(unspecified_matrix(height), Bt709, "{height}");
        }
    }

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
