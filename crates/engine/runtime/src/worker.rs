//! The dedicated thread shared by `Session` and `Forwarder`: a stop flag
//! with the [`Hangup`] it cuts, a reported phase, and a completion the async
//! side can await with a deadline.
use prysm_core::Frame;
use prysm_ports::{Error, FrameSource, Hangup, Pull, Result, Section, SessionPhase};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::oneshot;

/// How long a source may block before the loop rechecks its stop flag.
pub const TICK: Duration = Duration::from_millis(50);

#[derive(Default)]
pub(crate) struct Flags {
    stop: AtomicBool,
    /// Cut with the stop, after the flag is set: a blocking call it ends
    /// returns to a loop that sees the stop.
    stopping: Hangup,
    phase: Mutex<Option<SessionPhase>>,
}

impl Flags {
    fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
        self.stopping.cut();
    }

    /// Cut when the thread is asked to stop: the calls given it end then.
    pub(crate) fn stopping(&self) -> &Hangup {
        &self.stopping
    }

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
    ) -> Exit {
        self.set_phase(SessionPhase::Running);
        let pumped = guarded("session loop", || {
            while !self.stop.load(Ordering::Acquire) {
                // A failing source is the capture's failure.
                match source
                    .next_frame(TICK)
                    .map_err(|error| error.in_section(Section::Capture))?
                {
                    Pull::Frame(frame) => each(Some(frame))?,
                    Pull::Idle => each(None)?,
                    Pull::Ended => return Ok(Exit::Ended),
                }
            }
            Ok(Exit::Stopped)
        });
        pumped.unwrap_or_else(Exit::Failed)
    }
}

/// Run `call` with a panic turned into [`Error::Panicked`] naming `what`, so
/// the thread goes on to release what it holds and reports the panic.
pub(crate) fn guarded<T>(what: &'static str, call: impl FnOnce() -> Result<T>) -> Result<T> {
    catch_unwind(AssertUnwindSafe(call)).unwrap_or(Err(Error::Panicked(what)))
}

/// Why a pump loop returned.
#[derive(Debug)]
pub(crate) enum Exit {
    /// The stop flag was set: the thread was asked to stop.
    Stopped,
    /// The source reported [`Pull::Ended`].
    Ended,
    Failed(Error),
}

/// The worker's hold on the shared flags: dropping it requests stop, so a
/// worker dropped without [`Worker::stop`] or [`Worker::join`] still ends its
/// thread, which then releases what it holds on its own.
struct StopOnDrop(Arc<Flags>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.request_stop();
    }
}

pub(crate) struct Worker<T> {
    /// What the thread is in errors, such as "session thread".
    what: &'static str,
    flags: StopOnDrop,
    done: oneshot::Receiver<T>,
    thread: JoinHandle<()>,
}

impl<T: Send + 'static> Worker<T> {
    /// Run `body(state, flags)` on a new thread named `name`, called `what`
    /// in errors.
    ///
    /// # Errors
    /// The thread could not be spawned; `state` comes back with the error, so
    /// the caller can still release what it holds.
    pub(crate) fn spawn<S: Send + 'static>(
        name: &str,
        what: &'static str,
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
                return Err((Error::adapter(format!("cannot start {what}"), error), state));
            }
        };
        // The thread waits in `recv`, so this fails only if it panicked first.
        if let Err(mpsc::SendError(state)) = hand_over.send(state) {
            return Err((Error::Panicked(what), state));
        }
        Ok(Self {
            what,
            flags: StopOnDrop(flags),
            done,
            thread,
        })
    }

    fn phase(&self) -> SessionPhase {
        self.flags
            .0
            .phase
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .unwrap_or(SessionPhase::Running)
    }

    /// Wait for the thread to finish on its own. Cancel-safe. `Err` means the
    /// thread panicked outside the guarded loop.
    pub(crate) async fn finished(&mut self) -> Result<T> {
        let what = self.what;
        (&mut self.done).await.map_err(|_| Error::Panicked(what))
    }

    /// Request stop and join within `deadline`. On timeout the thread is left
    /// running and the phase it is stuck in is returned.
    pub(crate) async fn stop(mut self, deadline: Duration) -> Result<Result<T>, SessionPhase> {
        self.flags.0.request_stop();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_a_worker_asks_its_thread_to_stop() {
        let (stopped, seen) = mpsc::channel();
        let worker = Worker::spawn("prysm-test", "test thread", (), move |(), flags| {
            while !flags.stop.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(1));
            }
            let _ = stopped.send(());
        })
        .map_err(|(error, ())| error)
        .unwrap();
        drop(worker);
        assert!(seen.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    #[tokio::test]
    async fn a_thread_that_panics_is_named_in_the_error() {
        let mut worker = Worker::<()>::spawn("prysm-test", "test thread", (), |(), _| {
            panic!("on purpose")
        })
        .map_err(|(error, ())| error)
        .unwrap();
        assert!(matches!(
            worker.finished().await,
            Err(Error::Panicked("test thread"))
        ));
        worker.join().await;
    }
}
