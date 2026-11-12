//! One opened device. The native worker reports through async channels; a
//! private bridge thread drives them with `futures::executor::block_on` (no
//! timers, so no runtime) and publishes readiness, the newest frame and the
//! worker's exit into [`Shared`]. The session thread only ever waits on a
//! condvar with a deadline.
use crate::DeviceSettings;
use futures::{FutureExt, executor::block_on, future::BoxFuture};
use prysm_capture::{CaptureError, CaptureExit, DeviceCapture};
use prysm_core::Frame;
use prysm_ports::{Error, FrameSource, Result};
use std::num::NonZeroU32;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;

/// The native capture as the adapter consumes it. Private test seam: tests
/// build one from channels to drive readiness, frames, failure and release.
pub(crate) struct Native {
    /// Native activation, independent of the first frame. Closed: startup failed.
    pub ready: oneshot::Receiver<()>,
    /// Newest frame; closes when the worker exits.
    pub frames: watch::Receiver<Option<Frame>>,
    pub cancel: CancellationToken,
    /// The worker's join. `Err` means release could not be certified.
    pub join: BoxFuture<'static, Result<CaptureExit, CaptureError>>,
}

impl Native {
    pub(crate) fn open(settings: &DeviceSettings) -> Result<Self> {
        let fps =
            NonZeroU32::new(settings.fps).ok_or_else(|| Error::other("fps must be positive"))?;
        let DeviceCapture {
            frames,
            ready,
            mut worker,
        } = DeviceCapture::open(Some(&settings.path), settings.width, settings.height, fps)
            .map_err(|error| {
                Error::adapter(
                    format!("cannot open video device `{}`", settings.path),
                    error,
                )
            })?;
        Ok(Self {
            ready,
            frames,
            cancel: worker.cancellation(),
            // Dropping the worker unjoined cancels it.
            join: async move { worker.join().await }.boxed(),
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Deadlines {
    /// How long `open` waits for native activation.
    pub ready: Duration,
    /// How long `close` waits for the worker to release the device. Shorter
    /// than the runtime's session stop deadline so the adapter reports first.
    /// The native worker notices cancellation within its own poll timeout
    /// (V4L2: 1 s plus one frame period).
    pub release: Duration,
}

impl Deadlines {
    pub(crate) const DEVICE: Self = Self {
        ready: Duration::from_secs(10),
        release: Duration::from_secs(3),
    };
}

/// How the worker ended.
#[derive(Debug, Clone)]
struct Exit {
    /// Why capture stopped; `None` when it stopped cleanly.
    failure: Option<Error>,
    released: Result<()>,
}

impl From<Result<CaptureExit, CaptureError>> for Exit {
    fn from(joined: Result<CaptureExit, CaptureError>) -> Self {
        match joined {
            Ok(CaptureExit { failure }) => Self {
                failure: failure.map(|error| Error::adapter("video device capture failed", error)),
                released: Ok(()),
            },
            Err(error) => {
                let error = Error::adapter("video device release is unconfirmed", error);
                Self {
                    failure: Some(error.clone()),
                    released: Err(error),
                }
            }
        }
    }
}

#[derive(Default)]
struct State {
    ready: bool,
    frame: Option<Frame>,
    exit: Option<Exit>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn update(&self, change: impl FnOnce(&mut State)) {
        change(&mut self.state.lock().unwrap_or_else(PoisonError::into_inner));
        self.changed.notify_all();
    }

    fn wait(&self, timeout: Duration, until: impl Fn(&State) -> bool) -> MutexGuard<'_, State> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        self.changed
            .wait_timeout_while(state, timeout, |state| !until(state))
            .unwrap_or_else(PoisonError::into_inner)
            .0
    }
}

/// Owns the bridge thread, which owns the native worker.
fn bridge(native: Native, shared: &Shared) {
    let Native {
        ready,
        mut frames,
        join,
        ..
    } = native;
    if block_on(ready).is_ok() {
        shared.update(|state| state.ready = true);
        while block_on(frames.changed()).is_ok() {
            let frame = frames.borrow_and_update().clone();
            if let Some(frame) = frame {
                shared.update(|state| state.frame = Some(frame));
            }
        }
    }
    // Readiness or frames closed: the worker is exiting. Join it here so
    // `close` can certify release without driving the future itself.
    drop(frames);
    let exit = Exit::from(block_on(join));
    shared.update(|state| state.exit = Some(exit));
}

pub(crate) struct DeviceFrames {
    shared: Arc<Shared>,
    cancel: CancellationToken,
    bridge: Option<JoinHandle<()>>,
    release_within: Duration,
}

impl DeviceFrames {
    /// Wait for native readiness. On any failure the device is released first.
    pub(crate) fn start(native: Native, deadlines: Deadlines) -> Result<Self> {
        let shared = Arc::new(Shared::default());
        let cancel = native.cancel.clone();
        let thread = std::thread::Builder::new()
            .name("prysm-capture-bridge".into())
            .spawn({
                let shared = shared.clone();
                move || bridge(native, &shared)
            })
            .map_err(|error| {
                // The closure, and with it the worker, was dropped: cancelled, not joined.
                cancel.cancel();
                Error::adapter(
                    "cannot start capture bridge thread; video device release is unconfirmed",
                    error,
                )
            })?;
        let frames = Self {
            shared,
            cancel,
            bridge: Some(thread),
            release_within: deadlines.release,
        };
        let outcome = {
            let state = frames
                .shared
                .wait(deadlines.ready, |state| state.ready || state.exit.is_some());
            (!state.ready).then(|| {
                state.exit.as_ref().map(|exit| {
                    exit.failure
                        .clone()
                        .unwrap_or_else(|| Error::other("video device stopped before it was ready"))
                })
            })
        };
        let Some(failure) = outcome else {
            return Ok(frames);
        };
        let released = frames.release();
        Err(match (failure, released) {
            (Some(failure), _) => failure,
            (None, Ok(())) => Error::other(format!(
                "video device was not ready within {:?}",
                deadlines.ready
            )),
            (None, Err(unreleased)) => Error::adapter(
                format!("video device was not ready within {:?}", deadlines.ready),
                unreleased,
            ),
        })
    }

    /// The frame the bridge has published and nobody has taken yet.
    #[cfg(test)]
    pub(crate) fn pending_frame(&self) -> Option<Frame> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .frame
            .clone()
    }

    /// Whether the bridge has published the worker's exit.
    #[cfg(test)]
    pub(crate) fn exited(&self) -> bool {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .exit
            .is_some()
    }

    /// Cancel, then wait for the bridge to report the worker's join.
    fn release(mut self) -> Result<()> {
        self.cancel.cancel();
        let released = self
            .shared
            .wait(self.release_within, |state| state.exit.is_some())
            .exit
            .as_ref()
            .map(|exit| exit.released.clone());
        let Some(released) = released else {
            return Err(Error::other(format!(
                "video device release is unconfirmed: native capture worker did not stop within {:?}",
                self.release_within
            )));
        };
        // The exit is the bridge's last act, so this join is immediate.
        if let Some(thread) = self.bridge.take() {
            thread
                .join()
                .map_err(|_| Error::Panicked("capture bridge thread"))?;
        }
        released
    }
}

impl FrameSource for DeviceFrames {
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<Frame>> {
        let mut state = self.shared.wait(timeout, |state| {
            state.frame.is_some() || state.exit.is_some()
        });
        // An exit fences any frame not yet taken.
        if let Some(exit) = &state.exit {
            return Err(exit
                .failure
                .clone()
                .unwrap_or_else(|| Error::other("video device capture ended unexpectedly")));
        }
        Ok(state.frame.take())
    }

    fn close(self: Box<Self>) -> Result<()> {
        self.release()
    }
}

impl Drop for DeviceFrames {
    /// Without `close` the device is still cancelled, but release is not awaited.
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
