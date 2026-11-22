//! The native thread: owns the backend and its tap, follows the video signal,
//! paces acquisition, retries recoverable interruptions and publishes through
//! its [`Worker`].
use super::{Acquired, Backend, Fault, Limits, Recovery, Tap};
use crate::{Readiness, Video, VideoSignal, WebosSettings};
use prysm_ports::{Error, Result, Worker, chain};
use std::time::{Duration, Instant};

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
    /// It failed since its last frame (or since the start); the last wait.
    Failing(Duration),
    /// It cannot work in this process, or kept failing through the longest
    /// wait: reported once, never retried.
    Unsupported,
}

pub(super) struct Native<B> {
    /// Declared before `backend`: fields drop in order, so on a panic the
    /// tap's native cleanup still runs while the backend holds the device lock.
    tap: Option<(Kind, Box<dyn Tap>)>,
    backend: B,
    settings: WebosSettings,
    video: VideoSignal,
    limits: Limits,
    worker: Worker,
    /// The tap kind the video state asks for; `None`: idle.
    kind: Option<Kind>,
    ui: UiSupport,
    /// Earliest time to open or acquire again.
    retry_at: Instant,
    not_ready_since: Option<Instant>,
    pending_since: Option<Instant>,
    /// A video fault was logged since the last video frame or the start of
    /// this video period.
    video_failing: bool,
    /// [`Recovery::Bounded`] video faults in a row.
    unchecked: u32,
}

impl<B: Backend> Native<B> {
    pub(super) fn new(
        backend: B,
        settings: WebosSettings,
        video: VideoSignal,
        limits: Limits,
        worker: Worker,
    ) -> Self {
        Self {
            tap: None,
            backend,
            settings,
            video,
            limits,
            worker,
            kind: None,
            ui: UiSupport::Untried,
            retry_at: Instant::now(),
            not_ready_since: None,
            pending_since: None,
            video_failing: false,
            unchecked: 0,
        }
    }

    /// Capture until stopped or failed, release the tap and drop the
    /// backend; the release result is the thread's last act.
    pub(super) fn run(mut self) -> Result<()> {
        if let Err(error) = self.capture() {
            // Publish the failure before potentially blocking teardown.
            self.worker.fail(error);
        }
        let released = self.release_tap();
        drop(self);
        released
    }

    /// Capture until stopped or failed.
    fn capture(&mut self) -> Result<()> {
        let period = Duration::from_secs(1) / self.settings.fps.max(1);
        let mut generation = self.video.current().generation;
        let mut next = Instant::now();
        while !self.worker.stopped() {
            let Readiness {
                video,
                generation: current,
            } = self.video.current();
            let kind = self.wanted(video);
            // A change of tap kind or video context releases the current tap;
            // the next step opens the kind that matches.
            if kind != self.kind || current != generation {
                generation = current;
                self.kind = kind;
                // A new period: its first video failure is news again, and
                // its registration failures count from zero.
                self.video_failing = false;
                self.unchecked = 0;
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
                self.worker
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
            self.worker.sleep(next - now);
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
        let worker = self.worker.clone();
        let tap = match &mut self.tap {
            Some((_, tap)) => tap,
            None => {
                let opened = match kind {
                    Kind::Video => worker.watch("open", || self.backend.open(&self.settings)),
                    Kind::Ui => worker.watch("UI open", || self.backend.open_ui(&self.settings)),
                };
                match opened {
                    Ok(tap) => {
                        if kind == Kind::Video {
                            self.unchecked = 0;
                        }
                        &mut self.tap.insert((kind, tap)).1
                    }
                    Err(fault) => return self.recover(kind, fault),
                }
            }
        };
        let operation = match kind {
            Kind::Video => "acquire",
            Kind::Ui => "UI acquire",
        };
        let acquired = worker.watch(operation, || tap.acquire());
        self.acquired(kind, acquired)
    }

    /// Act on one acquisition: publish a frame, time a pending or preparing
    /// driver, or recover from a fault.
    fn acquired(&mut self, kind: Kind, acquired: Result<Acquired, Fault>) -> Result<()> {
        match acquired {
            Ok(Acquired::Frame(frame)) => {
                self.not_ready_since = None;
                self.pending_since = None;
                if kind == Kind::Video {
                    self.video_failing = false;
                }
                // The first frame, or the first after failures, ends the
                // backoff and is reported once.
                if kind == Kind::Ui && self.ui != UiSupport::Works {
                    self.ui = UiSupport::Works;
                    tracing::info!(
                        width = frame.width(),
                        height = frame.height(),
                        "capturing the TV UI while there is no video"
                    );
                }
                self.worker.offer(frame);
            }
            Ok(Acquired::Pending) => {
                let since = *self.pending_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= self.limits.pending {
                    return self.recover(
                        kind,
                        Fault::recoverable(Error::other(format!(
                            "webOS video produced no frame for {:?}",
                            self.limits.pending
                        ))),
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

    /// Fatal video faults end the source, and so does the
    /// [`Limits::unchecked`]th [`Recovery::Bounded`] one in a row; others
    /// release the tap and retry later. UI faults never end the source: a
    /// fatal one, or one after the longest wait, marks UI capture
    /// unsupported, reported once and not retried (lighting then idles
    /// without video, as with `include_ui` off); others retry with a backoff
    /// that a frame resets, so a long session without video regains the TV
    /// UI after a transient failure.
    fn recover(&mut self, kind: Kind, fault: Fault) -> Result<()> {
        let wait = match (kind, fault.recovery) {
            (Kind::Video, Recovery::Never) => return Err(fault.error),
            (Kind::Video, Recovery::Bounded) if self.unchecked + 1 >= self.limits.unchecked => {
                return Err(Error::adapter(
                    format!(
                        "the Luna registration check failed {} times in a row",
                        self.limits.unchecked
                    ),
                    fault.error,
                ));
            }
            (Kind::Video, recovery) => {
                // In a row: any other outcome starts the count again.
                self.unchecked = if recovery == Recovery::Bounded {
                    self.unchecked + 1
                } else {
                    0
                };
                let wait = self.limits.retry;
                Self::retrying(
                    &fault.error,
                    "webOS video capture",
                    !self.video_failing,
                    wait,
                );
                self.video_failing = true;
                wait
            }
            (Kind::Ui, recovery) => {
                let previous = match self.ui {
                    UiSupport::Failing(wait) => Some(wait),
                    _ => None,
                };
                let next = if recovery == Recovery::Never {
                    None
                } else {
                    self.limits.ui_backoff(previous)
                };
                match next {
                    None => {
                        self.ui = UiSupport::Unsupported;
                        tracing::warn!(
                            error = %chain(&fault.error),
                            "TV UI capture is unsupported; no frames while there is no video"
                        );
                        self.limits.retry
                    }
                    Some(wait) => {
                        Self::retrying(&fault.error, "TV UI capture", previous.is_none(), wait);
                        self.ui = UiSupport::Failing(wait);
                        wait
                    }
                }
            }
        };
        self.release_tap()?;
        self.retry_at = Instant::now() + wait;
        Ok(())
    }

    /// Logs that `tap` failed with `error` and is retried in `wait`: the
    /// `first` failure of a streak as a warning, the rest at debug, since
    /// only the first is news.
    fn retrying(error: &Error, tap: &str, first: bool, wait: Duration) {
        let error = chain(error);
        if first {
            tracing::warn!(%error, retry_in = ?wait, "{tap} failed; retrying");
        } else {
            tracing::debug!(%error, retry_in = ?wait, "{tap} failed; retrying");
        }
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
        self.worker
            .watch(operation, || tap.release())
            .map_err(|error| Error::adapter(unconfirmed, error))
    }
}
