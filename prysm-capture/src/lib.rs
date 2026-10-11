use futures::{Stream, StreamExt};
use std::fmt::{Display, Formatter};
use std::sync::Arc;
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;

pub mod yuyv;
pub use ::yuv::{YuvRange, YuvStandardMatrix};

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
    #[error("capture thread ended unexpectedly")]
    WorkerStopped(#[source] oneshot::error::RecvError),
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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    RGB24, // 3 bytes per pixel
    BGR24, // 3 bytes per pixel
    #[default]
    YUYV, // 2 bytes per pixel (4:2:2 subsampling)
    MJPEG, // Variable size (future support)
}

impl Display for PixelFormat {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            PixelFormat::RGB24 => f.write_str("RGB24"),
            PixelFormat::BGR24 => f.write_str("BGR24"),
            PixelFormat::YUYV => f.write_str("YUYV"),
            PixelFormat::MJPEG => f.write_str("JPEG"),
        }
    }
}

impl PixelFormat {
    /// Returns the bytes per pixel for this format.
    /// Returns None for variable-size formats like MJPEG.
    pub fn bytes_per_pixel(&self) -> Option<usize> {
        match self {
            PixelFormat::RGB24 | PixelFormat::BGR24 => Some(3),
            PixelFormat::YUYV => Some(2),
            PixelFormat::MJPEG => None,
        }
    }

    /// Returns the expected buffer size for a frame with the given dimensions.
    /// Returns None for variable-size formats like MJPEG.
    pub fn expected_size(&self, width: u32, height: u32) -> Option<usize> {
        self.bytes_per_pixel()
            .map(|bpp| (width as usize) * (height as usize) * bpp)
    }
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub data: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    /// YUYV color metadata; ignored for RGB and compressed formats.
    pub yuv_range: YuvRange,
    pub yuv_matrix: YuvStandardMatrix,
}

impl Frame {
    /// Validate a capture payload and remove row padding before constructing a frame.
    #[cfg(any(target_os = "linux", test))]
    fn from_strided_buffer(
        data: &[u8],
        width: u32,
        height: u32,
        format: PixelFormat,
        stride: usize,
        corrupted: bool,
    ) -> Option<Self> {
        if corrupted
            || width == 0
            || height == 0
            || (format == PixelFormat::YUYV && !width.is_multiple_of(2))
        {
            return None;
        }
        let row_size = (width as usize).checked_mul(format.bytes_per_pixel()?)?;
        let required = stride
            .checked_mul(height as usize - 1)?
            .checked_add(row_size)?;
        if stride < row_size || data.len() < required {
            return None;
        }
        let mut packed = Vec::with_capacity(row_size.checked_mul(height as usize)?);
        for row in 0..height as usize {
            packed.extend_from_slice(&data[row * stride..row * stride + row_size]);
        }
        Some(Self::new(packed, width, height, format))
    }

    /// Creates a new frame with the given data, dimensions, and pixel format.
    /// YUYV defaults to full-range BT.601; capturers set the negotiated metadata.
    ///
    /// # Panics
    /// Panics if the data size doesn't match the expected size for the given format and dimensions
    /// (except for variable-size formats like MJPEG), or if YUYV width is odd.
    pub fn new(data: Vec<u8>, width: u32, height: u32, format: PixelFormat) -> Self {
        assert!(
            format != PixelFormat::YUYV || width.is_multiple_of(2),
            "YUYV width must be even"
        );
        // Validate buffer size for fixed-size formats
        if let Some(expected) = format.expected_size(width, height) {
            assert_eq!(
                data.len(),
                expected,
                "Frame data size mismatch: expected {} bytes for {}x{} {:?}, got {}",
                expected,
                width,
                height,
                format,
                data.len()
            );
        }

        Self {
            data: Arc::new(data),
            width,
            height,
            format,
            yuv_range: YuvRange::Full,
            yuv_matrix: YuvStandardMatrix::Bt601,
        }
    }

    /// Create a frame filled with the specified byte value
    #[must_use]
    pub fn fill(value: u8, width: u32, height: u32, format: PixelFormat) -> Self {
        let expected_size = format.expected_size(width, height).unwrap();
        let data = vec![value; expected_size];
        Self::new(data, width, height, format)
    }

    /// Create a dummy frame (zero-filled, which appears green in YUYV)
    #[must_use]
    pub fn dummy(width: u32, height: u32) -> Self {
        Self::fill(0, width, height, PixelFormat::YUYV)
    }

    /// Returns a slice view of the frame data.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }

    /// Returns the size of the frame data in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Returns true if the frame data is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// Live capture yields the newest available frame, or a terminal capture error.
/// Intermediate frames may be skipped under load; cancellation closes the stream.
pub trait PrysmCapturer {
    fn into_stream(
        self,
        width: u32,
        height: u32,
    ) -> impl Stream<Item = Result<Frame>> + Send + 'static
    where
        Self: Sized + Send + 'static;
}

// Keep the terminal result separate so a final frame cannot overwrite a failure.
fn capture_channel(
    shutdown: CancellationToken,
) -> (
    watch::Sender<Option<Frame>>,
    oneshot::Sender<Result<()>>,
    impl Stream<Item = Result<Frame>> + Send,
) {
    let (sender, receiver) = watch::channel(None::<Frame>);
    let (finished, completion) = oneshot::channel::<Result<()>>();
    let frames = futures::stream::try_unfold(
        (receiver, completion),
        |(mut receiver, completion)| async move {
            while receiver.changed().await.is_ok() {
                let frame = receiver.borrow_and_update().clone();
                if let Some(frame) = frame {
                    return Ok(Some((frame, (receiver, completion))));
                }
            }
            completion.await.map_err(CaptureError::WorkerStopped)??;
            Ok(None)
        },
    );
    (
        sender,
        finished,
        frames.take_until(shutdown.cancelled_owned()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

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

    #[test]
    fn strided_capture_rejects_corruption_and_short_payloads() {
        let bytes = [16, 128, 235, 128, 0, 0, 16, 128, 235, 128];
        let frame = Frame::from_strided_buffer(&bytes, 2, 2, PixelFormat::YUYV, 6, false).unwrap();
        assert_eq!(frame.as_slice(), [16, 128, 235, 128].repeat(2));
        for (data, stride, corrupted) in [
            (&bytes[..], 6, true),
            (&bytes[..9], 6, false),
            (&bytes[..], 3, false),
            (&bytes[..], usize::MAX, false),
        ] {
            assert!(
                Frame::from_strided_buffer(data, 2, 2, PixelFormat::YUYV, stride, corrupted)
                    .is_none()
            );
        }
        assert!(Frame::from_strided_buffer(&bytes, 1, 2, PixelFormat::YUYV, 6, false).is_none());
        assert!(Frame::from_strided_buffer(&bytes, 2, 0, PixelFormat::YUYV, 6, false).is_none());
    }

    #[test]
    #[should_panic(expected = "YUYV width must be even")]
    fn odd_yuyv_width_is_rejected() {
        Frame::new(vec![128; 6], 3, 1, PixelFormat::YUYV);
    }

    #[tokio::test]
    async fn slow_consumer_gets_latest_frame_before_terminal_error() {
        let (sender, finished, frames) = capture_channel(CancellationToken::new());
        // Simulate a consumer paused for eight capture intervals.
        for value in 1..=8 {
            sender
                .send(Some(Frame::fill(value, 2, 1, PixelFormat::RGB24)))
                .unwrap();
        }
        finished
            .send(Err(CaptureError::Disconnected {
                details: "test camera".into(),
            }))
            .unwrap();
        drop(sender);
        futures::pin_mut!(frames);
        assert_eq!(frames.next().await.unwrap().unwrap().as_slice(), &[8; 6]);
        assert!(matches!(
            frames.next().await.unwrap().unwrap_err(),
            CaptureError::Disconnected { .. }
        ));
        assert!(frames.next().await.is_none());
    }

    #[tokio::test]
    async fn cancellation_and_consumer_drop_stop_capture() {
        let shutdown = CancellationToken::new();
        let (sender, _finished, frames) = capture_channel(shutdown.clone());
        let mut frames = Box::pin(frames);
        assert!(futures::poll!(frames.next()).is_pending());
        shutdown.cancel();
        assert!(frames.next().await.is_none());
        drop(frames);
        assert!(sender.is_closed());
        assert!(sender.send(Some(Frame::dummy(2, 1))).is_err());
    }

    #[tokio::test]
    async fn unexpected_capture_thread_exit_is_an_error() {
        let (sender, finished, frames) = capture_channel(CancellationToken::new());
        drop(sender);
        drop(finished);
        futures::pin_mut!(frames);
        assert!(matches!(
            frames.next().await.unwrap().unwrap_err(),
            CaptureError::WorkerStopped(_)
        ));
    }
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::V4lCapturer;
#[cfg(target_os = "linux")]
pub use linux::V4lCapturer as Capturer;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::AVFoundationCapturer;
#[cfg(target_os = "macos")]
pub use macos::AVFoundationCapturer as Capturer;
