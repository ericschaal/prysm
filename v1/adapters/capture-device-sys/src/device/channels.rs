//! [`capture`] on its own thread behind tokio channels, for async callers.
use super::{CaptureSink, capture};
use crate::{CaptureError, Frame, Result};
use std::num::NonZeroU32;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::{oneshot, watch};

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

pub struct CaptureWorker {
    stop: Arc<Stop>,
    finished: oneshot::Receiver<()>,
    join: Option<JoinHandle<Result<()>>>,
}

#[derive(Default)]
struct Stop {
    requested: Mutex<bool>,
    changed: Condvar,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Capture stops on [`CaptureWorker::cancel`] or once every frame receiver is gone.
struct Channels {
    frames: watch::Sender<Option<Frame>>,
    ready: Mutex<Option<oneshot::Sender<()>>>,
    stop: Arc<Stop>,
}

impl CaptureSink for Channels {
    fn ready(&self) {
        if let Some(ready) = lock(&self.ready).take() {
            let _ = ready.send(());
        }
    }

    fn frame(&self, frame: Frame) {
        let _ = self.frames.send(Some(frame));
    }

    fn should_stop(&self, wait: Duration) -> bool {
        let requested = *self
            .stop
            .changed
            .wait_timeout_while(lock(&self.stop.requested), wait, |requested| !*requested)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
        requested || self.frames.is_closed()
    }
}

impl DeviceCapture {
    pub fn open(device: Option<&str>, width: u32, height: u32, rate: NonZeroU32) -> Result<Self> {
        prysm_core::CaptureResolution::new(width, height)
            .map_err(|error| CaptureError::Backend(anyhow::anyhow!(error)))?;
        let device = device.map(str::to_owned);
        Self::spawn(move |sink| capture(device.as_deref(), width, height, rate, &sink))
    }

    fn spawn(
        run: impl FnOnce(Arc<dyn CaptureSink>) -> Result<()> + Send + 'static,
    ) -> Result<Self> {
        let (sender, frames) = watch::channel(None);
        let (ready_sender, ready) = oneshot::channel();
        let (done, finished) = oneshot::channel();
        let stop = Arc::new(Stop::default());
        // Readiness and frames close when capture drops the sink.
        let sink = Arc::new(Channels {
            frames: sender,
            ready: Mutex::new(Some(ready_sender)),
            stop: stop.clone(),
        });
        let join = std::thread::Builder::new()
            .name("prysm-capture".into())
            .spawn(move || {
                let result = run(sink);
                let _ = done.send(());
                result
            })
            .map_err(|error| CaptureError::Backend(error.into()))?;
        Ok(Self {
            frames,
            ready,
            worker: CaptureWorker {
                stop,
                finished,
                join: Some(join),
            },
        })
    }
}

impl CaptureWorker {
    pub fn cancel(&self) {
        *lock(&self.stop.requested) = true;
        self.stop.changed.notify_all();
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

    #[tokio::test]
    async fn readiness_does_not_need_frames_and_cancelled_join_retains_cleanup() {
        let (release, released) = std::sync::mpsc::channel();
        let mut capture = DeviceCapture::spawn(move |sink| {
            sink.ready();
            while !sink.should_stop(Duration::from_secs(5)) {}
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
    async fn dropping_the_frames_stops_capture() {
        let mut capture = DeviceCapture::spawn(|sink| {
            sink.frame(Frame::dummy(2, 1).unwrap());
            while !sink.should_stop(Duration::from_millis(10)) {}
            Ok(())
        })
        .unwrap();
        drop(capture.frames);
        let exit = tokio::time::timeout(Duration::from_secs(5), capture.worker.join())
            .await
            .unwrap();
        assert!(exit.unwrap().failure.is_none());
    }

    #[tokio::test]
    async fn operational_failure_and_cleanup_panic_are_distinct() {
        let mut capture = DeviceCapture::spawn(|_| {
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
        let mut capture = DeviceCapture::spawn(|_| panic!("cleanup failed")).unwrap();
        assert!(matches!(
            capture.worker.join().await,
            Err(CaptureError::Cleanup(_))
        ));
        let mut capture = DeviceCapture::spawn(|_| {
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
