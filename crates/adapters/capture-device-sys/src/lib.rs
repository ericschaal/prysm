//! Capture interfaces, latest-frame delivery, and platform adapters.
pub use prysm_core::Frame;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use prysm_core::{FramePool, PixelFormat, YuvRange, YuvStandardMatrix};
use prysm_ports::{Escalation, Fix};
mod device;
#[cfg(any(target_os = "linux", target_os = "macos", test))]
mod formats;
pub use device::{CaptureSink, DeviceInfo, capture, list_devices, list_formats};

/// Terminal capture failures. Cancellation closes the stream successfully.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CaptureError {
    /// `device` names the one asked for: a path on Linux, a unique ID on
    /// macOS; `None` asked for the default.
    #[error("{}", missing_device(.device.as_deref()))]
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
    /// Another program holds the device or its buffers.
    #[error("capture device is in use by another program: {details}")]
    Busy {
        details: String,
        #[source]
        source: std::io::Error,
    },
    /// Linux: the process may not open the device node.
    #[error("no permission to open {device} (add the user to group video)")]
    Denied {
        device: String,
        #[source]
        source: std::io::Error,
    },
    #[error("unsupported capture format: {details}")]
    UnsupportedFormat {
        details: String,
        #[source]
        source: Option<std::io::Error>,
    },
    /// macOS has not granted the process camera access, which capture cards
    /// need too; the device is not opened.
    #[error("camera access is not granted")]
    NoCameraAccess(#[source] CameraAccess),
    /// The device kept delivering buffers without a gap, none of them
    /// usable, for several seconds; `source` is why the last one was not.
    #[error("capture device delivered {rejected} unusable buffers in a row")]
    Unusable {
        rejected: u32,
        #[source]
        source: BufferRejection,
    },
    #[error("native capture cleanup failed: {0}")]
    Cleanup(#[source] anyhow::Error),
    #[error(transparent)]
    Backend(#[from] anyhow::Error),
}

impl CaptureError {
    /// What is left for the person once this failure ends capture.
    #[must_use]
    pub fn escalation(&self) -> Escalation {
        match self {
            Self::DeviceNotFound { .. } => Escalation::Ask(Fix::ChangeSettings),
            Self::Denied { .. } | Self::NoCameraAccess(CameraAccess::Denied) => {
                Escalation::Ask(Fix::SetUp)
            }
            // The device may still hold buffers or the fd: only a new
            // process frees them.
            Self::Cleanup(_) => Escalation::Restart,
            Self::Disconnected { .. }
            | Self::Busy { .. }
            | Self::UnsupportedFormat { .. }
            | Self::NoCameraAccess(
                CameraAccess::Restricted | CameraAccess::NotAsked | CameraAccess::Unanswered,
            )
            | Self::Unusable { .. }
            | Self::Backend(_) => Escalation::FAIL_FAST,
        }
    }
}

fn missing_device(device: Option<&str>) -> String {
    device.map_or_else(
        || "no video capture device found".to_owned(),
        |device| format!("no video capture device at {device}"),
    )
}

/// Why macOS refuses camera access, each with what the user can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CameraAccess {
    #[error(
        "allow the app that starts Prysm (such as Terminal) under System Settings > \
         Privacy & Security > Camera, then restart that app"
    )]
    Denied,
    #[error("this Mac's administrator restricts camera access")]
    Restricted,
    /// macOS answered the request without asking: the app that started Prysm
    /// cannot show it.
    #[error("macOS did not ask; start Prysm from an app that can ask (such as Terminal)")]
    NotAsked,
    /// Nobody answered macOS's request in time. Nothing retries by itself, so
    /// the message says to start the lights again.
    #[error("macOS got no answer to its request in time; allow access and start the lights again")]
    Unanswered,
}

/// How long opening on macOS waits for an answer to the camera access
/// request: long enough to read the request and answer it. It is shorter
/// than the device adapter's readiness deadline (the adapter asserts this),
/// so no answer is reported as [`CameraAccess::Unanswered`] rather than a
/// timeout. After a late answer, session start gets the time that remains.
pub const CAMERA_ACCESS_WAIT: std::time::Duration = std::time::Duration::from_secs(7);

/// Why a buffer the device delivered could not be used as a frame.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BufferRejection {
    /// The payload itself: flagged, short, or not the negotiated geometry.
    #[error(transparent)]
    Payload(#[from] prysm_core::PayloadError),
    /// An AVFoundation sample without a pixel buffer.
    #[error("sample held no pixel buffer")]
    NoPixelBuffer,
    /// An AVFoundation pixel buffer in a format other than YUYV (its
    /// four-character code).
    #[error("pixel format {} is not YUYV", String::from_utf8_lossy(.0))]
    PixelFormat([u8; 4]),
    /// An AVFoundation YUYV buffer whose colour matrix (an H.273 code point)
    /// has no conversion.
    #[error("YUYV colour matrix {0} is not supported")]
    Matrix(i32),
    /// An AVFoundation pixel buffer laid out in planes or with extended
    /// pixels, so its base address is not its first pixel.
    #[error("pixel buffer is not one plane of pixels")]
    Layout,
    /// An AVFoundation pixel buffer whose memory could not be locked for
    /// reading.
    #[error("pixel buffer memory could not be read")]
    Unreadable,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_device_reads_naturally_with_and_without_a_request() {
        let requested = CaptureError::DeviceNotFound {
            device: Some("/dev/video0".into()),
            source: Some(std::io::Error::from_raw_os_error(2)),
        };
        assert_eq!(
            prysm_ports::chain(&requested),
            "no video capture device at /dev/video0: No such file or directory (os error 2)"
        );
        let default = CaptureError::DeviceNotFound {
            device: None,
            source: None,
        };
        assert_eq!(default.to_string(), "no video capture device found");
    }

    #[test]
    fn each_capture_failure_escalates_as_its_cause_says() {
        let io = || Some(std::io::Error::from_raw_os_error(2));
        let rejection = BufferRejection::Payload(prysm_core::PayloadError::Corrupted);
        for (error, escalation) in [
            (
                CaptureError::DeviceNotFound {
                    device: None,
                    source: io(),
                },
                Escalation::Ask(Fix::ChangeSettings),
            ),
            (
                CaptureError::Disconnected {
                    details: "unplugged".into(),
                    source: io(),
                },
                Escalation::FAIL_FAST,
            ),
            (
                CaptureError::Busy {
                    details: "failed to set capture format".into(),
                    source: std::io::Error::from_raw_os_error(16),
                },
                Escalation::FAIL_FAST,
            ),
            (
                CaptureError::Denied {
                    device: "/dev/video0".into(),
                    source: std::io::Error::from_raw_os_error(13),
                },
                Escalation::Ask(Fix::SetUp),
            ),
            (
                CaptureError::UnsupportedFormat {
                    details: "no YUYV".into(),
                    source: io(),
                },
                Escalation::FAIL_FAST,
            ),
            (
                CaptureError::NoCameraAccess(CameraAccess::Denied),
                Escalation::Ask(Fix::SetUp),
            ),
            (
                CaptureError::NoCameraAccess(CameraAccess::Restricted),
                Escalation::FAIL_FAST,
            ),
            (
                CaptureError::NoCameraAccess(CameraAccess::NotAsked),
                Escalation::FAIL_FAST,
            ),
            (
                CaptureError::NoCameraAccess(CameraAccess::Unanswered),
                Escalation::FAIL_FAST,
            ),
            (
                CaptureError::Unusable {
                    rejected: 90,
                    source: rejection,
                },
                Escalation::FAIL_FAST,
            ),
            (
                CaptureError::Cleanup(anyhow::anyhow!("REQBUFS(0) failed")),
                Escalation::Restart,
            ),
            (
                CaptureError::Backend(anyhow::anyhow!("STREAMON failed")),
                Escalation::FAIL_FAST,
            ),
        ] {
            assert_eq!(error.escalation(), escalation, "{error}");
        }
    }

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
        let corrupted = BufferRejection::Payload(prysm_core::PayloadError::Corrupted);
        let unusable = CaptureError::Unusable {
            rejected: 90,
            source: corrupted.clone(),
        };
        let unusable = anyhow::Error::new(unusable).context("starting camera");
        assert_eq!(
            unusable
                .chain()
                .find_map(|cause| cause.downcast_ref::<BufferRejection>()),
            Some(&corrupted)
        );
        assert_eq!(
            BufferRejection::PixelFormat(*b"BGRA").to_string(),
            "pixel format BGRA is not YUYV"
        );
        let refused = CaptureError::NoCameraAccess(CameraAccess::Denied);
        let refused = anyhow::Error::new(refused).context("starting camera");
        assert_eq!(
            refused
                .chain()
                .find_map(|cause| cause.downcast_ref::<CameraAccess>()),
            Some(&CameraAccess::Denied)
        );
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
mod capture_time;
#[cfg(any(target_os = "linux", test))]
mod classify;
#[cfg(any(target_os = "linux", target_os = "macos", test))]
mod health;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(target_os = "linux", target_os = "macos", test))]
mod trace_stats;

#[cfg(target_os = "macos")]
mod macos;
