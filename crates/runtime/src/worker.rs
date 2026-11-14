//! The dedicated thread shared by `Session` and `Forwarder`: a stop flag, a
//! reported phase, and a completion the async side can await with a deadline.
use prysm_core::Frame;
use prysm_ports::{Error, FrameSource, Pull, Result, Section, SessionPhase};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::oneshot;

/// How long a source may block before the loop rechecks its stop flag.
pub(crate) const TICK: Duration = Duration::from_millis(50);

#[derive(Default)]
pub(crate) struct Flags {
    stop: AtomicBool,
    phase: Mutex<Option<SessionPhase>>,
}

impl Flags {
    pub(crate) fn set_phase(&self, phase: SessionPhase) {
        *self.phase.lock().unwrap_or_else(PoisonError::into_inner) = Some(phase);
    }

    /// Pull frames until stopped, until the source ends, or until the source
    /// or `each` fails; `each` receives `None` when no frame arrived within a
    /// tick. A panic becomes a failure so the caller still releases its adapters.
    pub(crate) fn pump(
        &self,
        source: &mut dyn FrameSource,
        mut each: impl FnMut(Option<Frame>) -> Result<()>,
    ) -> Pumped {
        self.set_phase(SessionPhase::Running);
        let pumped = catch_unwind(AssertUnwindSafe(|| {
            while !self.stop.load(Ordering::Acquire) {
                // A failing source is the capture's failure.
                match source
                    .next_frame(TICK)
                    .map_err(|error| error.in_section(Section::Capture))?
                {
                    Pull::Frame(frame) => each(Some(frame))?,
                    Pull::Idle => each(None)?,
                    Pull::Ended => return Ok(Pumped::Ended),
                }
            }
            Ok(Pumped::Stopped)
        }));
        match pumped {
            Ok(Ok(pumped)) => pumped,
            Ok(Err(error)) => Pumped::Failed(error),
            Err(_) => Pumped::Failed(Error::Panicked("session loop")),
        }
    }
}

/// Why a pump loop returned.
pub(crate) enum Pumped {
    /// The stop flag was set.
    Stopped,
    /// The source reported [`Pull::Ended`].
    Ended,
    Failed(Error),
}

impl Pumped {
    pub(crate) fn failure(self) -> Option<Error> {
        match self {
            Self::Failed(error) => Some(error),
            Self::Stopped | Self::Ended => None,
        }
    }
}

pub(crate) struct Worker<T> {
    flags: Arc<Flags>,
    done: oneshot::Receiver<T>,
    thread: JoinHandle<()>,
}

impl<T: Send + 'static> Worker<T> {
    /// Run `body(state, flags)` on a new thread named `name`.
    ///
    /// # Errors
    /// The thread could not be spawned; `state` comes back with the error, so
    /// the caller can still release what it holds.
    pub(crate) fn spawn<S: Send + 'static>(
        name: &str,
        state: S,
        body: impl FnOnce(S, &Flags) -> T + Send + 'static,
    ) -> Result<Self, (Error, S)> {
        let flags = Arc::new(Flags::default());
        let (sender, done) = oneshot::channel();
        // `state` is handed over only once the thread exists, since a failed
        // spawn drops the closure and everything it owns.
        let (hand_over, handed) = mpsc::sync_channel(1);
        let spawned = std::thread::Builder::new().name(name.into()).spawn({
            let flags = flags.clone();
            move || {
                let Ok(state) = handed.recv() else {
                    return;
                };
                let end = body(state, &flags);
                flags.set_phase(SessionPhase::Finishing);
                let _ = sender.send(end);
            }
        });
        let thread = match spawned {
            Ok(thread) => thread,
            Err(error) => {
                return Err((Error::adapter("cannot start session thread", error), state));
            }
        };
        // The thread waits in `recv`, so this fails only if it panicked first.
        if let Err(mpsc::SendError(state)) = hand_over.send(state) {
            return Err((Error::Panicked("session thread"), state));
        }
        Ok(Self {
            flags,
            done,
            thread,
        })
    }

    pub(crate) fn request_stop(&self) {
        self.flags.stop.store(true, Ordering::Release);
    }

    fn phase(&self) -> SessionPhase {
        self.flags
            .phase
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .unwrap_or(SessionPhase::Running)
    }

    /// Wait for the thread to finish on its own. Cancel-safe. `Err` means the
    /// thread panicked outside the guarded loop.
    pub(crate) async fn finished(&mut self) -> Result<T> {
        (&mut self.done)
            .await
            .map_err(|_| Error::Panicked("session thread"))
    }

    /// Request stop and join within `deadline`. On timeout the thread is left
    /// running and the phase it is stuck in is returned.
    pub(crate) async fn stop(mut self, deadline: Duration) -> Result<Result<T>, SessionPhase> {
        self.request_stop();
        match tokio::time::timeout(deadline, self.finished()).await {
            Ok(end) => {
                self.join().await;
                Ok(end)
            }
            Err(_) => Err(self.phase()),
        }
    }

    /// Join a thread that has finished or is about to; never blocks the runtime.
    pub(crate) async fn join(self) {
        let thread = self.thread;
        let _ = tokio::task::spawn_blocking(move || thread.join()).await;
    }
}
