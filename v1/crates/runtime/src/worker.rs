//! The dedicated thread shared by `Session` and `Forwarder`: a stop flag, a
//! reported phase, and a completion the async side can await with a deadline.
use prysm_core::Frame;
use prysm_ports::{Error, FrameSource, Result, SessionPhase};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
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

    /// Pull frames until stopped or until the source or `each` fails; `each`
    /// receives `None` when no frame arrived within a tick. A panic
    /// becomes a failure so the caller still releases its adapters.
    pub(crate) fn pump(
        &self,
        source: &mut dyn FrameSource,
        mut each: impl FnMut(Option<Frame>) -> Result<()>,
    ) -> Option<Error> {
        self.set_phase(SessionPhase::Running);
        let pumped = catch_unwind(AssertUnwindSafe(|| {
            while !self.stop.load(Ordering::Acquire) {
                match source.next_frame(TICK) {
                    Ok(frame) => each(frame)?,
                    Err(error) => return Err(error),
                }
            }
            Ok(())
        }));
        match pumped {
            Ok(result) => result.err(),
            Err(_) => Some(Error::Panicked("session loop")),
        }
    }
}

pub(crate) struct Worker<T> {
    flags: Arc<Flags>,
    done: oneshot::Receiver<T>,
    thread: JoinHandle<()>,
}

impl<T: Send + 'static> Worker<T> {
    pub(crate) fn spawn(
        name: &str,
        body: impl FnOnce(&Flags) -> T + Send + 'static,
    ) -> Result<Self> {
        let flags = Arc::new(Flags::default());
        let (sender, done) = oneshot::channel();
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn({
                let flags = flags.clone();
                move || {
                    let end = body(&flags);
                    flags.set_phase(SessionPhase::Finished);
                    let _ = sender.send(end);
                }
            })
            .map_err(|error| Error::adapter("cannot start session thread", error))?;
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
