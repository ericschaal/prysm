//! Native device lifetime. Readiness and release never depend on frame consumption.
use crate::{CaptureError, Frame, Result};
use std::{num::NonZeroU32, thread::JoinHandle};
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;

pub struct DeviceCapture {
    pub frames: watch::Receiver<Option<Frame>>,
    /// Successful native activation, independent of first-frame arrival.
    /// Closure means startup did not succeed; join carries the cause.
    pub ready: oneshot::Receiver<()>,
    pub worker: CaptureWorker,
}

/// A joined worker whose native resources were released, possibly after a capture failure.
#[derive(Debug)]
pub struct CaptureExit {
    pub failure: Option<CaptureError>,
}

/// A video capture device present on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// The name the device gives itself, such as `USB Video`.
    pub name: String,
    /// What [`DeviceCapture::open`] takes: the V4L2 node on Linux, the
    /// `AVFoundation` unique ID on macOS.
    pub id: String,
}

/// The video capture devices present, without starting any: V4L2 asks each
/// `/dev/video*` node for its capabilities, `AVFoundation` lists its devices.
/// Empty on other platforms.
#[must_use]
pub fn list_devices() -> Vec<DeviceInfo> {
    #[cfg(target_os = "macos")]
    return crate::macos::list_devices();
    #[cfg(target_os = "linux")]
    return crate::linux::list_devices();
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    Vec::new()
}

pub struct CaptureWorker {
    cancel: CancellationToken,
    finished: oneshot::Receiver<()>,
    join: Option<JoinHandle<Result<()>>>,
}

impl DeviceCapture {
    pub fn open(device: Option<&str>, width: u32, height: u32, rate: NonZeroU32) -> Result<Self> {
        prysm_core::CaptureResolution::new(width, height)
            .map_err(|error| CaptureError::Backend(anyhow::anyhow!(error)))?;
        let device = device.map(str::to_owned);
        Self::spawn(move |frames, ready, cancel| {
            #[cfg(target_os = "macos")]
            return crate::macos::run(
                device.as_deref(),
                width,
                height,
                rate,
                frames,
                ready,
                cancel,
            );
            #[cfg(target_os = "linux")]
            return crate::linux::run(
                device.as_deref(),
                width,
                height,
                rate,
                &frames,
                ready,
                &cancel,
            );
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            {
                let _ = (device, width, height, rate, frames, ready, cancel);
                Err(CaptureError::Backend(anyhow::anyhow!(
                    "video device capture is unsupported on this platform"
                )))
            }
        })
    }

    fn spawn(
        run: impl FnOnce(
            watch::Sender<Option<Frame>>,
            oneshot::Sender<()>,
            CancellationToken,
        ) -> Result<()>
        + Send
        + 'static,
    ) -> Result<Self> {
        let (sender, frames) = watch::channel(None);
        let (ready_sender, ready) = oneshot::channel();
        let (done, finished) = oneshot::channel();
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let join = std::thread::Builder::new()
            .name("prysm-capture".into())
            .spawn(move || {
                let result = run(sender, ready_sender, worker_cancel);
                let _ = done.send(());
                result
            })
            .map_err(|error| CaptureError::Backend(error.into()))?;
        Ok(Self {
            frames,
            ready,
            worker: CaptureWorker {
                cancel,
                finished,
                join: Some(join),
            },
        })
    }
}

impl CaptureWorker {
    #[must_use]
    pub fn cancellation(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Cancel-safe until completion. Call once to consume the retained thread result.
    /// An outer error means release could not be certified; do not reopen the device.
    ///
    /// # Panics
    /// Panics if called again after returning a result.
    pub async fn join(&mut self) -> Result<CaptureExit> {
        let _ = (&mut self.finished).await;
        let result = self
            .join
            .take()
            .expect("capture worker joined once")
            .join()
            .map_err(|panic| {
                let message = panic
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| panic.downcast_ref::<&str>().copied())
                    .unwrap_or("unknown panic");
                CaptureError::Cleanup(anyhow::anyhow!("capture worker panicked: {message}"))
            })?;
        match result {
            Err(error @ CaptureError::Cleanup(_)) => Err(error),
            result => Ok(CaptureExit {
                failure: result.err(),
            }),
        }
    }
}

impl Drop for CaptureWorker {
    fn drop(&mut self) {
        // Emergency cancellation is not a cleanup acknowledgment; hosts retain and join owners.
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn readiness_does_not_need_frames_and_cancelled_join_retains_cleanup() {
        let (release, released) = std::sync::mpsc::channel();
        let mut capture = DeviceCapture::spawn(move |_frames, ready, cancel| {
            ready.send(()).unwrap();
            futures::executor::block_on(cancel.cancelled());
            released.recv().unwrap();
            Ok(())
        })
        .unwrap();
        (&mut capture.ready).await.unwrap();
        assert!(capture.frames.borrow().is_none());
        capture.worker.cancel();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), capture.worker.join())
                .await
                .is_err()
        );
        release.send(()).unwrap();
        assert!(capture.worker.join().await.unwrap().failure.is_none());
    }

    #[tokio::test]
    async fn operational_failure_and_cleanup_panic_are_distinct() {
        let mut capture = DeviceCapture::spawn(|_, _, _| {
            Err(CaptureError::DeviceNotFound {
                device: None,
                source: None,
            })
        })
        .unwrap();
        assert!((&mut capture.ready).await.is_err());
        assert!(matches!(
            capture.worker.join().await.unwrap().failure,
            Some(CaptureError::DeviceNotFound { .. })
        ));
        let mut capture = DeviceCapture::spawn(|_, _, _| panic!("cleanup failed")).unwrap();
        assert!(matches!(
            capture.worker.join().await,
            Err(CaptureError::Cleanup(_))
        ));
        let mut capture = DeviceCapture::spawn(|_, _, _| {
            Err(CaptureError::Cleanup(anyhow::anyhow!(
                "buffer release failed"
            )))
        })
        .unwrap();
        assert!(matches!(
            capture.worker.join().await,
            Err(CaptureError::Cleanup(_))
        ));
    }
}
