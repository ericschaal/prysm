//! Capture interfaces, latest-frame delivery, and platform adapters.
pub use prysm_core::{
    Frame, FrameError, FramePool, PixelFormat, YuvRange, YuvStandardMatrix, yuyv,
};
#[cfg(feature = "remote")]
mod receiver;
#[cfg(feature = "remote")]
mod webos;
#[cfg(feature = "remote")]
pub use receiver::WebosReceiver;
mod device;
#[cfg(feature = "async")]
pub use device::{CaptureExit, CaptureWorker, DeviceCapture};
pub use device::{CaptureSink, DeviceInfo, capture, list_devices};

#[cfg(feature = "remote")]
use futures::{Stream, StreamExt};
#[cfg(feature = "remote")]
use tokio::sync::{oneshot, watch};
#[cfg(feature = "remote")]
use tokio_util::sync::CancellationToken;

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
    #[cfg(feature = "remote")]
    #[error("capture thread ended unexpectedly")]
    WorkerStopped(#[source] oneshot::error::RecvError),
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

// Keep the terminal result separate so a final frame cannot overwrite a failure.
#[cfg(feature = "remote")]
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

    #[cfg(feature = "remote")]
    #[tokio::test]
    async fn slow_consumer_gets_latest_frame_before_terminal_error() {
        let (sender, finished, frames) = capture_channel(CancellationToken::new());
        // Simulate a consumer paused for eight capture intervals.
        for value in 1..=8 {
            sender
                .send(Some(Frame::fill(value, 2, 1, PixelFormat::RGB24).unwrap()))
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

    #[cfg(feature = "remote")]
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
        assert!(sender.send(Some(Frame::dummy(2, 1).unwrap())).is_err());
    }

    #[cfg(feature = "remote")]
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

#[cfg(any(target_os = "linux", target_os = "macos", test))]
mod capture_time;
#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "macos")]
mod macos;
