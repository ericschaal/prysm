//! One opened webOS source. A private native thread owns the backend and its
//! tap (LG requires load, capture and release on one thread). It follows the
//! [`VideoSignal`] to pick the tap kind (video, or the TV UI plane when there
//! is no video and `include_ui` is on; never both), paces acquisition,
//! retries recoverable interruptions and publishes the newest frame, a terminal failure and the release result into
//! [`Shared`]. The session thread only waits on a condvar with a deadline and
//! enforces the native-operation watchdog, so a hung driver call surfaces as
//! an error instead of a hang.
use crate::{Video, VideoSignal, WebosSettings};
use prysm_core::Frame;
use prysm_ports::{Error, FrameSource, Pull, Result};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

// ---- Native seam -------------------------------------------------------

/// One acquisition attempt.
pub(crate) enum Acquired {
    Frame(Frame),
    /// No new buffer yet.
    Pending,
    /// The driver is (re)initializing; try again after [`Limits::retry`].
    NotReady,
}

/// A native failure. Recoverable ones release the tap and reopen it.
#[derive(Debug)]
pub(crate) struct Fault {
    pub error: Error,
    pub fatal: bool,
}

/// An open native tap. Created, used and released on the native thread.
pub(crate) trait Tap {
    fn acquire(&mut self) -> Result<Acquired, Fault>;
    /// `Ok` certifies the driver released the tap.
    fn release(self: Box<Self>) -> Result<()>;
}

/// Opens taps. Moved to the native thread and dropped there after the last
/// tap's release, so anything it holds (the device lock) outlives the tap.
pub(crate) trait Backend: Send + 'static {
    /// The video tap (blended with menus when `include_ui` is on).
    fn open(&mut self, settings: &WebosSettings) -> Result<Box<dyn Tap>, Fault>;
    /// The TV UI (graphics plane) tap, used only while there is no video.
    fn open_ui(&mut self, settings: &WebosSettings) -> Result<Box<dyn Tap>, Fault>;
}

/// Which tap the native thread holds. The two never run together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Video,
    Ui,
}

/// Whether the TV UI tap works in this source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiSupport {
    Untried,
    Works,
    /// It failed before its first frame: reported once, never retried.
    Unsupported,
}

// ---- Timing ------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// Longest a single native call may run before the source fails.
    pub watchdog: Duration,
    /// Longest the driver may stay [`Acquired::NotReady`].
    pub preparation: Duration,
    /// Longest the driver may stay [`Acquired::Pending`] before the tap is restarted.
    pub pending: Duration,
    /// Delay before reopening after a recoverable failure or between
    /// preparation attempts.
    pub retry: Duration,
    /// How long `close` waits for certified release. Below the forwarder's
    /// 5 s and the engine's 10 s stop deadlines so the adapter reports first,
    /// and below the supervisor's 5 s TERM-to-KILL window.
    pub release: Duration,
    /// How often an idle native thread rechecks the video signal.
    pub poll: Duration,
}

impl Limits {
    pub(crate) const TV: Self = Self {
        watchdog: Duration::from_secs(5),
        preparation: Duration::from_secs(10),
        pending: Duration::from_secs(2),
        retry: Duration::from_secs(1),
        release: Duration::from_secs(4),
        poll: Duration::from_millis(100),
    };
}

// ---- Shared state ------------------------------------------------------

#[derive(Default)]
struct State {
    cancelled: bool,
    frame: Option<Frame>,
    /// The native call in progress and when it started.
    busy: Option<(&'static str, Instant)>,
    /// Terminal failure; published before native teardown.
    failure: Option<Error>,
    /// The native thread's final release result; its last act.
    released: Option<Result<()>>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
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

    fn stopping(&self) -> bool {
        let state = self.lock();
        state.cancelled || state.failure.is_some()
    }

    /// Sleep, waking early when the source stops.
    fn sleep(&self, duration: Duration) {
        drop(self.wait(duration, |state| state.cancelled || state.failure.is_some()));
    }

    /// Run one native call under the watchdog.
    fn watch<T>(&self, operation: &'static str, call: impl FnOnce() -> T) -> T {
        self.update(|state| state.busy = Some((operation, Instant::now())));
        let result = call();
        self.update(|state| state.busy = None);
        result
    }

    /// Record the first terminal failure.
    fn fail(&self, error: Error) {
        self.update(|state| {
            state.failure.get_or_insert(error);
        });
    }
}

// ---- Native thread -----------------------------------------------------

struct Native<B> {
    backend: B,
    settings: WebosSettings,
    video: VideoSignal,
    limits: Limits,
    shared: Arc<Shared>,
    /// The tap kind the video state asks for; `None`: idle.
    kind: Option<Kind>,
    tap: Option<(Kind, Box<dyn Tap>)>,
    ui: UiSupport,
    /// Earliest time to open or acquire again.
    retry_at: Instant,
    not_ready_since: Option<Instant>,
    pending_since: Option<Instant>,
}

impl<B: Backend> Native<B> {
    fn run(mut self) {
        let released = match self.capture() {
            Ok(()) => self.release_tap(),
            Err(error) => {
                // Publish the failure before potentially blocking teardown.
                self.shared.fail(error);
                self.release_tap()
            }
        };
        let Self {
            backend, shared, ..
        } = self;
        drop(backend);
        shared.update(|state| state.released = Some(released));
    }

    /// Capture until cancelled or failed.
    fn capture(&mut self) -> Result<()> {
        let period = Duration::from_secs(1) / self.settings.fps.max(1);
        let mut generation = self.video.current().1;
        let mut next = Instant::now();
        while !self.shared.stopping() {
            let (video, current) = self.video.current();
            let kind = self.wanted(video);
            // A change of tap kind or video context releases the current tap;
            // the next step opens the kind that matches.
            if kind != self.kind || current != generation {
                generation = current;
                self.kind = kind;
                self.release_tap()?;
                self.retry_at = Instant::now();
            }
            let now = Instant::now();
            let Some(kind) = kind.filter(|_| now >= self.retry_at) else {
                // Idle: recheck the video signal every poll; retrying: wake
                // at the retry time (at most a poll away).
                let wake = match kind {
                    None => self.limits.poll,
                    Some(_) => self.retry_at.saturating_duration_since(now),
                };
                self.shared
                    .sleep(wake.clamp(Duration::from_millis(1), self.limits.poll));
                continue;
            };
            self.step(kind)?;
            // Pace on a fixed clock; skip missed ticks instead of bursting.
            next += period;
            let now = Instant::now();
            if now > next {
                next = now;
            }
            self.shared.sleep(next - now);
        }
        Ok(())
    }

    /// The tap for this video state: video while it is available or
    /// unknown, the TV UI without video (if enabled and supported), else none.
    fn wanted(&self, video: Video) -> Option<Kind> {
        match video {
            Video::Available | Video::Unknown => Some(Kind::Video),
            Video::Unavailable if self.settings.include_ui && self.ui != UiSupport::Unsupported => {
                Some(Kind::Ui)
            }
            Video::Unavailable => None,
        }
    }

    /// Open if needed, then acquire once.
    fn step(&mut self, kind: Kind) -> Result<()> {
        let shared = self.shared.clone();
        if self.tap.is_none() {
            let opened = match kind {
                Kind::Video => shared.watch("open", || self.backend.open(&self.settings)),
                Kind::Ui => shared.watch("UI open", || self.backend.open_ui(&self.settings)),
            };
            match opened {
                Ok(tap) => self.tap = Some((kind, tap)),
                Err(fault) => return self.recover(kind, fault),
            }
        }
        let Some((_, tap)) = self.tap.as_mut() else {
            return Ok(());
        };
        let operation = match kind {
            Kind::Video => "acquire",
            Kind::Ui => "UI acquire",
        };
        match shared.watch(operation, || tap.acquire()) {
            Ok(Acquired::Frame(frame)) => {
                self.not_ready_since = None;
                self.pending_since = None;
                if kind == Kind::Ui && self.ui == UiSupport::Untried {
                    self.ui = UiSupport::Works;
                    tracing::info!(
                        width = frame.width(),
                        height = frame.height(),
                        "capturing the TV UI while there is no video"
                    );
                }
                shared.update(|state| state.frame = Some(frame));
            }
            Ok(Acquired::Pending) => {
                let since = *self.pending_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= self.limits.pending {
                    return self.recover(
                        kind,
                        Fault {
                            error: Error::other(format!(
                                "webOS video produced no frame for {:?}",
                                self.limits.pending
                            )),
                            fatal: false,
                        },
                    );
                }
            }
            Ok(Acquired::NotReady) => {
                let since = *self.not_ready_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= self.limits.preparation {
                    return Err(Error::other(format!(
                        "webOS video was not ready within {:?}",
                        self.limits.preparation
                    )));
                }
                self.retry_at = Instant::now() + self.limits.retry;
            }
            Err(fault) => return self.recover(kind, fault),
        }
        Ok(())
    }

    /// Fatal video faults end the source; others release the tap and retry
    /// later. UI faults never end the source; one before the UI tap's first
    /// frame marks UI capture unsupported, so it is reported once and not
    /// retried (lighting then idles without video, as with `include_ui` off).
    fn recover(&mut self, kind: Kind, fault: Fault) -> Result<()> {
        if kind == Kind::Ui && self.ui == UiSupport::Untried {
            self.ui = UiSupport::Unsupported;
            tracing::warn!(
                error = %chain(&fault.error),
                "TV UI capture is unsupported; no frames while there is no video"
            );
        } else if kind == Kind::Video && fault.fatal {
            return Err(fault.error);
        }
        self.release_tap()?;
        self.retry_at = Instant::now() + self.limits.retry;
        Ok(())
    }

    fn release_tap(&mut self) -> Result<()> {
        self.not_ready_since = None;
        self.pending_since = None;
        let Some((kind, tap)) = self.tap.take() else {
            return Ok(());
        };
        let (operation, unconfirmed) = match kind {
            Kind::Video => ("release", "webOS video release is unconfirmed"),
            Kind::Ui => ("UI release", "webOS UI capture release is unconfirmed"),
        };
        self.shared
            .watch(operation, || tap.release())
            .map_err(|error| Error::adapter(unconfirmed, error))
    }
}

/// An error and its causes on one line, for the log.
fn chain(error: &Error) -> String {
    let mut text = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

// ---- The source --------------------------------------------------------

/// The TV's video as a [`FrameSource`]: the newest frame, `Ok(None)` while
/// video is unavailable or the driver is preparing, and `Err` on a terminal
/// native failure or a native call that exceeds its watchdog.
pub struct WebosFrames {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    limits: Limits,
}

impl std::fmt::Debug for WebosFrames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebosFrames").finish_non_exhaustive()
    }
}

impl WebosFrames {
    /// Start the native thread. It opens the tap once video allows it.
    pub(crate) fn start(
        backend: impl Backend,
        settings: WebosSettings,
        video: VideoSignal,
        limits: Limits,
    ) -> Result<Self> {
        let shared = Arc::new(Shared::default());
        let thread = std::thread::Builder::new()
            .name("prysm-webos-capture".into())
            .spawn({
                let shared = shared.clone();
                // Taps are not `Send`; they only ever exist on this thread.
                move || {
                    Native {
                        backend,
                        settings,
                        video,
                        limits,
                        shared,
                        kind: None,
                        tap: None,
                        ui: UiSupport::Untried,
                        retry_at: Instant::now(),
                        not_ready_since: None,
                        pending_since: None,
                    }
                    .run();
                }
            })
            // Nothing native was opened; the backend dropped with the closure.
            .map_err(|error| Error::adapter("cannot start webOS capture thread", error))?;
        Ok(Self {
            shared,
            thread: Some(thread),
            limits,
        })
    }

    fn cancel(&self) {
        self.shared.update(|state| state.cancelled = true);
    }

    /// A native call past its watchdog becomes the terminal failure.
    fn check_watchdog(&self, state: &mut State) {
        if state.failure.is_none()
            && let Some((operation, started)) = state.busy
            && started.elapsed() >= self.limits.watchdog
        {
            state.failure = Some(Error::other(format!(
                "native webOS {operation} exceeded its {:?} watchdog",
                self.limits.watchdog
            )));
            self.shared.changed.notify_all();
        }
    }

    /// The frame published and not yet taken.
    #[cfg(test)]
    pub(crate) fn pending_frame(&self) -> Option<Frame> {
        self.shared.lock().frame.clone()
    }

    fn release(mut self) -> Result<()> {
        self.cancel();
        let state = self
            .shared
            .wait(self.limits.release, |state| state.released.is_some());
        let Some(released) = state.released.clone() else {
            let busy = state.busy.map_or_else(String::new, |(operation, started)| {
                format!("; native {operation} has run for {:?}", started.elapsed())
            });
            return Err(Error::other(format!(
                "webOS video release is unconfirmed: capture thread did not stop within {:?}{busy}",
                self.limits.release
            )));
        };
        drop(state);
        // Publishing the release is the thread's last act, so this is immediate.
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| Error::Panicked("webOS capture thread"))?;
        }
        released
    }
}

impl FrameSource for WebosFrames {
    fn next_frame(&mut self, timeout: Duration) -> Result<Pull> {
        let mut state = self.shared.wait(timeout, |state| {
            state.frame.is_some() || state.failure.is_some() || state.released.is_some()
        });
        self.check_watchdog(&mut state);
        if let Some(failure) = &state.failure {
            return Err(failure.clone());
        }
        if state.released.is_some() {
            return Err(Error::other("webOS capture ended unexpectedly"));
        }
        if self.thread.as_ref().is_some_and(JoinHandle::is_finished) {
            return Err(Error::Panicked("webOS capture thread"));
        }
        Ok(state.frame.take().into())
    }

    fn close(self: Box<Self>) -> Result<()> {
        self.release()
    }
}

impl Drop for WebosFrames {
    /// Without `close` the tap is still released, but not awaited.
    fn drop(&mut self) {
        self.cancel();
    }
}
