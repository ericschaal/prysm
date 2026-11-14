use crate::{Error, Pull, Result};
use prysm_core::Frame;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// The thread that owns a push-style source's native resources.
///
/// The thread runs one closure with a [`Worker`]: it publishes readiness, the
/// newest frame (an untaken one is replaced, never queued) and a terminal
/// failure, checks [`Worker::stopped`], and returns its release result as its
/// last act. The source's session thread only waits here with a deadline:
/// [`Self::next_frame`] for frames, [`Self::close`] to stop the thread and
/// certify release. Dropping without `close` stops the thread and waits for
/// it within the same release bound.
pub struct OwnerThread {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    /// The thread in messages, such as `webOS capture thread`.
    what: &'static str,
    release: Duration,
    watchdog: Option<Duration>,
}

/// The owner thread's side: what it publishes and when it stops.
#[derive(Clone)]
pub struct Worker {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct State {
    stop: bool,
    ready: bool,
    frame: Option<Frame>,
    /// The watched native call in progress and when it started.
    busy: Option<(&'static str, Instant)>,
    /// The first terminal failure.
    failure: Option<Error>,
    /// The thread's release result; its last act.
    released: Option<Result<()>>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    // A panicking holder cannot leave the state half-written.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn update<T>(&self, change: impl FnOnce(&mut State) -> T) -> T {
        let result = change(&mut self.lock());
        self.changed.notify_all();
        result
    }

    fn wait(&self, timeout: Duration, until: impl Fn(&State) -> bool) -> MutexGuard<'_, State> {
        self.changed
            .wait_timeout_while(self.lock(), timeout, |state| !until(state))
            .unwrap_or_else(PoisonError::into_inner)
            .0
    }
}

impl OwnerThread {
    /// Run `run` on a new thread called `name`. `run` works until
    /// [`Worker::stopped`], releases what it holds and returns the release
    /// result: `Ok` certifies release. A panic is a failure and leaves
    /// release unconfirmed. `close` waits up to `release` for that result.
    ///
    /// # Errors
    /// The thread cannot be spawned; `run` was dropped without running.
    pub fn spawn(
        name: &str,
        what: &'static str,
        release: Duration,
        run: impl FnOnce(&Worker) -> Result<()> + Send + 'static,
    ) -> Result<Self> {
        let shared = Arc::new(Shared::default());
        let worker = Worker {
            shared: shared.clone(),
        };
        let thread = thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let released = panic::catch_unwind(AssertUnwindSafe(|| run(&worker)))
                    .unwrap_or_else(|_| {
                        worker.fail(Error::Panicked(what));
                        Err(Error::Panicked(what))
                    });
                worker
                    .shared
                    .update(|state| state.released = Some(released));
            })
            .map_err(|error| Error::adapter(format!("cannot start the {what}"), error))?;
        Ok(Self {
            shared,
            thread: Some(thread),
            what,
            release,
            watchdog: None,
        })
    }

    /// Fail the source when one [`Worker::watch`]ed call runs longer than `limit`.
    #[must_use]
    pub fn with_watchdog(mut self, limit: Duration) -> Self {
        self.watchdog = Some(limit);
        self
    }

    /// Wait up to `timeout` for [`Worker::ready`]: `Ok(false)` if it has not
    /// come yet.
    ///
    /// # Errors
    /// The thread failed or ended before it was ready.
    pub fn wait_ready(&self, timeout: Duration) -> Result<bool> {
        let state = self.shared.wait(timeout, |state| {
            state.ready || state.failure.is_some() || state.released.is_some()
        });
        if state.ready {
            return Ok(true);
        }
        match (&state.failure, &state.released) {
            (Some(failure), _) => Err(failure.clone()),
            (None, Some(_)) => Err(self.ended()),
            (None, None) => Ok(false),
        }
    }

    /// Wait up to `timeout` for a frame and take it; [`Pull::Idle`] without one.
    ///
    /// # Errors
    /// The terminal failure (also a watched call past its watchdog), on every
    /// call once it happened, even if a frame was left untaken; or the thread
    /// ended without one.
    pub fn next_frame(&mut self, timeout: Duration) -> Result<Pull> {
        let mut state = self.shared.wait(timeout, |state| {
            state.frame.is_some() || state.failure.is_some() || state.released.is_some()
        });
        self.check_watchdog(&mut state);
        if let Some(failure) = &state.failure {
            return Err(failure.clone());
        }
        if state.released.is_some() {
            return Err(self.ended());
        }
        Ok(state.frame.take().into())
    }

    /// The frame published and not yet taken.
    #[must_use]
    pub fn peek(&self) -> Option<Frame> {
        self.shared.lock().frame.clone()
    }

    /// Ask the thread to stop, without waiting.
    pub fn stop(&self) {
        self.shared.update(|state| state.stop = true);
    }

    /// Stop the thread and wait up to the release bound for its result.
    ///
    /// # Errors
    /// The thread's release failed, or it did not finish in time (release
    /// is unconfirmed and the thread is left running), or it panicked.
    pub fn close(mut self) -> Result<()> {
        self.finish()
    }

    fn finish(&mut self) -> Result<()> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        self.stop();
        let state = self
            .shared
            .wait(self.release, |state| state.released.is_some());
        let Some(released) = state.released.clone() else {
            let busy = state.busy.map_or_else(String::new, |(operation, started)| {
                format!("; native {operation} has run for {:?}", started.elapsed())
            });
            return Err(Error::other(format!(
                "{} did not stop within {:?}, so release is unconfirmed{busy}",
                self.what, self.release
            )));
        };
        drop(state);
        // Publishing the release is the thread's last act, so this is immediate.
        thread.join().map_err(|_| Error::Panicked(self.what))?;
        released
    }

    fn ended(&self) -> Error {
        Error::other(format!("{} ended unexpectedly", self.what))
    }

    /// A watched call past its watchdog becomes the terminal failure.
    fn check_watchdog(&self, state: &mut State) {
        if state.failure.is_none()
            && let Some(limit) = self.watchdog
            && let Some((operation, started)) = state.busy
            && started.elapsed() >= limit
        {
            state.failure = Some(Error::other(format!(
                "native {operation} in the {} exceeded its {limit:?} watchdog",
                self.what
            )));
            self.shared.changed.notify_all();
        }
    }
}

impl std::fmt::Debug for OwnerThread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnerThread")
            .field("what", &self.what)
            .finish_non_exhaustive()
    }
}

impl Drop for OwnerThread {
    /// Without `close` the release result is lost; the thread is still
    /// stopped and joined within the release bound.
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

impl Worker {
    /// Whether to stop: `close` or drop asked, or a failure was published.
    #[must_use]
    pub fn stopped(&self) -> bool {
        let state = self.shared.lock();
        state.stop || state.failure.is_some()
    }

    /// Sleep up to `duration`, waking early when stopped.
    pub fn sleep(&self, duration: Duration) {
        drop(
            self.shared
                .wait(duration, |state| state.stop || state.failure.is_some()),
        );
    }

    /// Native activation succeeded; [`OwnerThread::wait_ready`] returns.
    pub fn ready(&self) {
        self.shared.update(|state| state.ready = true);
    }

    /// Publish `frame`, replacing one not yet taken.
    pub fn offer(&self, frame: Frame) {
        self.shared.update(|state| state.frame = Some(frame));
    }

    /// Publish a terminal failure; the first one is kept.
    pub fn fail(&self, error: Error) {
        self.shared.update(|state| {
            state.failure.get_or_insert(error);
        });
    }

    /// Run one native call under the owner's watchdog, if it has one.
    pub fn watch<T>(&self, operation: &'static str, call: impl FnOnce() -> T) -> T {
        self.shared
            .update(|state| state.busy = Some((operation, Instant::now())));
        let result = call();
        self.shared.update(|state| state.busy = None);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_core::PixelFormat;

    const LIMIT: Duration = Duration::from_secs(5);
    const SHORT: Duration = Duration::from_millis(50);

    fn frame(value: u8) -> Frame {
        Frame::fill(value, 2, 1, PixelFormat::RGB24).unwrap()
    }

    fn spawn(run: impl FnOnce(&Worker) -> Result<()> + Send + 'static) -> OwnerThread {
        OwnerThread::spawn("prysm-test-owner", "test thread", LIMIT, run).unwrap()
    }

    fn until_stopped(worker: &Worker) {
        while !worker.stopped() {
            worker.sleep(LIMIT);
        }
    }

    #[test]
    fn newest_frame_wins_and_close_returns_the_release() {
        let mut owner = spawn(|worker| {
            worker.ready();
            for value in 1..=3 {
                worker.offer(frame(value));
            }
            until_stopped(worker);
            Err(Error::other("buffers stuck"))
        });
        assert!(owner.wait_ready(LIMIT).unwrap());
        let mut newest = None;
        while newest.is_none() {
            newest = owner.next_frame(LIMIT).unwrap().into_frame();
        }
        // Frames may be taken before the last offer; never an older one after a newer.
        let mut last = newest.unwrap().as_slice()[0];
        while last < 3 {
            let frame = owner.next_frame(LIMIT).unwrap().into_frame().unwrap();
            assert!(frame.as_slice()[0] > last);
            last = frame.as_slice()[0];
        }
        assert!(owner.next_frame(SHORT).unwrap().is_idle());
        assert_eq!(owner.close().unwrap_err().to_string(), "buffers stuck");
    }

    #[test]
    fn failure_fences_frames_and_stops_the_worker() {
        let mut owner = spawn(|worker| {
            worker.offer(frame(1));
            worker.fail(Error::other("unplugged"));
            worker.fail(Error::other("later"));
            assert!(worker.stopped());
            Ok(())
        });
        assert_eq!(
            owner.wait_ready(LIMIT).unwrap_err().to_string(),
            "unplugged"
        );
        for _ in 0..2 {
            assert_eq!(
                owner.next_frame(LIMIT).unwrap_err().to_string(),
                "unplugged"
            );
        }
        owner.close().unwrap();
    }

    #[test]
    fn ending_without_failure_or_readiness_is_unexpected() {
        let mut owner = spawn(|_| Ok(()));
        let error = owner.wait_ready(LIMIT).unwrap_err();
        assert_eq!(error.to_string(), "test thread ended unexpectedly");
        assert!(owner.next_frame(LIMIT).is_err());
        owner.close().unwrap();
    }

    #[test]
    fn readiness_times_out_without_an_error() {
        let owner = spawn(|worker| {
            until_stopped(worker);
            Ok(())
        });
        assert!(!owner.wait_ready(SHORT).unwrap());
        owner.close().unwrap();
    }

    #[test]
    fn a_panic_is_a_failure_and_an_unconfirmed_release() {
        let mut owner = spawn(|_| panic!("native bug"));
        assert!(matches!(
            owner.next_frame(LIMIT),
            Err(Error::Panicked("test thread"))
        ));
        assert!(matches!(owner.close(), Err(Error::Panicked("test thread"))));
    }

    #[test]
    fn close_is_bounded_and_names_the_hung_call() {
        let (hang, hung) = std::sync::mpsc::channel::<()>();
        let owner = OwnerThread::spawn("prysm-test-owner", "test thread", SHORT, move |worker| {
            worker.watch("release", || hung.recv().ok());
            Ok(())
        })
        .unwrap()
        .with_watchdog(SHORT);
        thread::sleep(SHORT * 2);
        let error = owner.close().unwrap_err().to_string();
        assert!(error.contains("release is unconfirmed"), "{error}");
        assert!(error.contains("native release has run"), "{error}");
        drop(hang);
    }

    #[test]
    fn watchdog_fails_a_hung_call() {
        let (hang, hung) = std::sync::mpsc::channel::<()>();
        let mut owner = spawn(move |worker| {
            worker.watch("acquire", || hung.recv().ok());
            Ok(())
        })
        .with_watchdog(SHORT);
        thread::sleep(SHORT * 2);
        let error = owner.next_frame(Duration::ZERO).unwrap_err().to_string();
        assert!(
            error.contains("native acquire in the test thread exceeded"),
            "{error}"
        );
        drop(hang);
        owner.close().unwrap();
    }

    #[test]
    fn drop_stops_and_joins() {
        let (done, finished) = std::sync::mpsc::channel();
        let owner = spawn(move |worker| {
            until_stopped(worker);
            done.send(()).unwrap();
            Ok(())
        });
        drop(owner);
        finished.try_recv().expect("drop waited for the thread");
    }
}
