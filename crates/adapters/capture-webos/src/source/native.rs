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

impl UiSupport {
    /// After a TV UI fault: failing again with the next wait of its streak
    /// ([`Limits::ui_backoff`]), or unsupported once it cannot heal or the
    /// longest wait did not help.
    fn after_fault(self, recovery: Recovery, limits: &Limits) -> Self {
        let previous = match self {
            Self::Failing(wait) => Some(wait),
            Self::Untried | Self::Works | Self::Unsupported => None,
        };
        match recovery {
            Recovery::Never => Self::Unsupported,
            Recovery::Reopen | Recovery::Bounded => limits
                .ui_backoff(previous)
                .map_or(Self::Unsupported, Self::Failing),
        }
    }
}

/// The tap for `video`: video while it is available or unknown, the TV UI
/// without video while `include_ui` and the UI is not unsupported, else none.
fn wanted(video: Video, include_ui: bool, ui: UiSupport) -> Option<Kind> {
    match video {
        Video::Available | Video::Unknown => Some(Kind::Video),
        Video::Unavailable if include_ui && ui != UiSupport::Unsupported => Some(Kind::Ui),
        Video::Unavailable => None,
    }
}

/// After a video fault, with `unchecked` [`Recovery::Bounded`] faults in a
/// row before it: the new count when the tap is reopened, or `None` when
/// the source fails (a fatal fault, or the [`Limits::unchecked`]th bounded
/// one in a row). Any other outcome starts the count again.
fn video_fault(unchecked: u32, recovery: Recovery, limits: &Limits) -> Option<u32> {
    match recovery {
        Recovery::Never => None,
        Recovery::Bounded if unchecked + 1 >= limits.unchecked => None,
        Recovery::Bounded => Some(unchecked + 1),
        Recovery::Reopen => Some(0),
    }
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
        wanted(video, self.settings.include_ui, self.ui)
    }

    /// Open if needed, then acquire once. The outcome is judged at the time
    /// the native call returned: a failing open can block for seconds.
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
                    Err(fault) => return self.recover(kind, fault, Instant::now()),
                }
            }
        };
        let operation = match kind {
            Kind::Video => "acquire",
            Kind::Ui => "UI acquire",
        };
        let acquired = worker.watch(operation, || tap.acquire());
        self.acquired(kind, acquired, Instant::now())
    }

    /// Act on one acquisition at `now`: publish a frame, time a pending or
    /// preparing driver, or recover from a fault.
    fn acquired(
        &mut self,
        kind: Kind,
        acquired: Result<Acquired, Fault>,
        now: Instant,
    ) -> Result<()> {
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
                let since = *self.pending_since.get_or_insert(now);
                if now.duration_since(since) >= self.limits.pending {
                    return self.recover(
                        kind,
                        Fault::recoverable(Error::other(format!(
                            "webOS video produced no frame for {:?}",
                            self.limits.pending
                        ))),
                        now,
                    );
                }
            }
            Ok(Acquired::NotReady) => {
                let since = *self.not_ready_since.get_or_insert(now);
                if now.duration_since(since) >= self.limits.preparation {
                    return Err(Error::other(format!(
                        "webOS video was not ready within {:?}",
                        self.limits.preparation
                    )));
                }
                self.retry_at = now + self.limits.retry;
            }
            Err(fault) => return self.recover(kind, fault, now),
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
    fn recover(&mut self, kind: Kind, fault: Fault, now: Instant) -> Result<()> {
        let wait = match kind {
            Kind::Video => {
                let Some(unchecked) = video_fault(self.unchecked, fault.recovery, &self.limits)
                else {
                    return Err(match fault.recovery {
                        Recovery::Bounded => Error::adapter(
                            format!(
                                "the Luna registration check failed {} times in a row",
                                self.limits.unchecked
                            ),
                            fault.error,
                        ),
                        Recovery::Never | Recovery::Reopen => fault.error,
                    });
                };
                self.unchecked = unchecked;
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
            Kind::Ui => {
                let first = !matches!(self.ui, UiSupport::Failing(_));
                self.ui = self.ui.after_fault(fault.recovery, &self.limits);
                match self.ui {
                    UiSupport::Failing(wait) => {
                        Self::retrying(&fault.error, "TV UI capture", first, wait);
                        wait
                    }
                    _ => {
                        tracing::warn!(
                            error = %chain(&fault.error),
                            "TV UI capture is unsupported; no frames while there is no video"
                        );
                        self.limits.retry
                    }
                }
            }
        };
        self.release_tap()?;
        self.retry_at = now + wait;
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

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: Limits = Limits {
        ui_retry: Duration::from_secs(5),
        ui_retry_max: Duration::from_secs(20),
        unchecked: 3,
        ..Limits::TV
    };
    const RECOVERIES: [Recovery; 3] = [Recovery::Never, Recovery::Reopen, Recovery::Bounded];

    fn secs(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    /// Every UI support state meets every kind of UI fault. The waits are
    /// 5, 10 and 20 s here; a fault after the 20 s wait is the last.
    #[test]
    fn every_ui_state_meets_every_fault() {
        use UiSupport::{Failing, Unsupported, Untried, Works};
        let states = [
            Untried,
            Works,
            Failing(secs(5)),
            Failing(secs(10)),
            Failing(secs(20)),
            Unsupported,
        ];
        // Per state: after a `Never`, `Reopen` and `Bounded` fault.
        let expected = [
            [Unsupported, Failing(secs(5)), Failing(secs(5))],
            [Unsupported, Failing(secs(5)), Failing(secs(5))],
            [Unsupported, Failing(secs(10)), Failing(secs(10))],
            [Unsupported, Failing(secs(20)), Failing(secs(20))],
            [Unsupported, Unsupported, Unsupported],
            [Unsupported, Failing(secs(5)), Failing(secs(5))],
        ];
        for (state, after) in states.into_iter().zip(expected) {
            for (recovery, wanted) in RECOVERIES.into_iter().zip(after) {
                assert_eq!(
                    state.after_fault(recovery, &LIMITS),
                    wanted,
                    "{state:?} meets {recovery:?}"
                );
            }
        }
    }

    /// An unsupported UI is never opened again; video always is.
    #[test]
    fn the_tap_follows_the_video_the_setting_and_the_ui_state() {
        use UiSupport::{Failing, Unsupported, Untried, Works};
        for ui in [Untried, Works, Failing(secs(5)), Unsupported] {
            for include_ui in [false, true] {
                for video in [Video::Available, Video::Unknown] {
                    assert_eq!(wanted(video, include_ui, ui), Some(Kind::Video));
                }
                let ui_tap = (include_ui && ui != Unsupported).then_some(Kind::Ui);
                assert_eq!(
                    wanted(Video::Unavailable, include_ui, ui),
                    ui_tap,
                    "{ui:?}, include_ui {include_ui}"
                );
            }
        }
    }

    /// Bounded faults in a row end the source at the third; a reopenable
    /// one starts the count again; a fatal one ends it at once.
    #[test]
    fn every_video_fault_count_meets_every_fault() {
        let cases = [
            (0, [None, Some(0), Some(1)]),
            (1, [None, Some(0), Some(2)]),
            (2, [None, Some(0), None]),
        ];
        for (unchecked, after) in cases {
            for (recovery, wanted) in RECOVERIES.into_iter().zip(after) {
                assert_eq!(
                    video_fault(unchecked, recovery, &LIMITS),
                    wanted,
                    "{unchecked} in a row meets {recovery:?}"
                );
            }
        }
    }
}
