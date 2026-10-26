//! Capture interfaces, latest-frame delivery, and platform adapters.
mod frame;
pub use frame::{Frame, PixelFormat};
mod webos;
pub use webos::WebosCapturer;

use futures::{Stream, StreamExt};
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
