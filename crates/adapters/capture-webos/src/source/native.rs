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

/// Whether the TV UI tap works in this source. A UI fault that cannot heal
/// makes it unsupported until the next source. Any other one stops the
/// tries until the video signal changes (video comes back, or the context
/// changes): a UI tap can fail for good (on the bench TV, a worker that died
/// with the UI tap open left the graphics driver's lock held, and every
/// later UI open failed until a reboot), so it is tried again only after an
/// event, never on a timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiSupport {
    Untried,
    Works,
    /// It failed in a way that may pass: not opened again until the video
    /// signal changes.
    Failed,
    /// It cannot work in this process: never opened again in this source.
    Unsupported,
}

impl UiSupport {
    /// After a TV UI fault.
    fn after_fault(recovery: Recovery) -> Self {
        match recovery {
            Recovery::Never => Self::Unsupported,
            Recovery::Reopen => Self::Failed,
        }
    }

    /// After the video signal changed: a failed UI may be tried again.
    fn after_change(self) -> Self {
        match self {
            Self::Failed => Self::Untried,
            other => other,
        }
    }

    fn may_open(self) -> bool {
        matches!(self, Self::Untried | Self::Works)
    }
}

/// When a video tap reopens after a recoverable fault. A TV UI tap reopens
/// only after an event ([`UiSupport`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reopen {
    /// At once: the fault is a stall, which already waited
    /// [`Limits::pending`], so a reopen cannot spin.
    AtOnce,
    /// After [`Limits::retry`], so a tap that fails at once is not reopened
    /// in a tight loop.
    AfterRetry,
}

/// After a video fault: how long until the tap reopens, or `None` when the
/// source fails.
fn video_wait(recovery: Recovery, reopen: Reopen, limits: &Limits) -> Option<Duration> {
    match (recovery, reopen) {
        (Recovery::Never, _) => None,
        (Recovery::Reopen, Reopen::AtOnce) => Some(Duration::ZERO),
        (Recovery::Reopen, Reopen::AfterRetry) => Some(limits.retry),
    }
}

/// The tap for `video`: video while it is available or unknown, the TV UI
/// without video while `include_ui` and the UI may open, else none.
fn wanted(video: Video, include_ui: bool, ui: UiSupport) -> Option<Kind> {
    match video {
        Video::Available | Video::Unknown => Some(Kind::Video),
        Video::Unavailable if include_ui && ui.may_open() => Some(Kind::Ui),
        Video::Unavailable => None,
    }
}

/// What a video acquisition says of the driver's initialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// `vtCapture_init` refused (status 17).
    Refused,
    /// Not ready yet (status 1).
    NotReady,
    /// Initialized: a frame or a pending buffer.
    Accepted,
    /// A native failure.
    Failed,
}

impl Answer {
    fn of(acquired: &Result<Acquired, Fault>) -> Self {
        match acquired {
            Ok(Acquired::Refused) => Self::Refused,
            Ok(Acquired::NotReady) => Self::NotReady,
            Ok(Acquired::Frame(_) | Acquired::Pending) => Self::Accepted,
            Err(_) => Self::Failed,
        }
    }
}

/// A refusal period of the held video driver: from an initialization the
/// driver refused to the first it accepts, a native failure, or video
/// becoming unavailable. Its pulls are refused meanwhile.
#[derive(Debug)]
struct Refusal {
    since: Instant,
    /// Acquisitions in this period, each an initialization of the created
    /// driver, the refused one that started it included.
    initializations: u32,
    /// The last initialization was refused: the next waits for a change of
    /// the video signal, since nothing tells when the device would allow
    /// it sooner.
    waiting: bool,
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
    /// A UI fault was logged since the last UI frame.
    ui_failing: bool,
    /// Started and ended only where the pulls are told.
    refusal: Option<Refusal>,
    /// When the video signal last changed, as the capture thread saw it.
    changed_at: Instant,
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
            ui_failing: false,
            refusal: None,
            changed_at: Instant::now(),
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

    /// Capture until stopped or failed. A change of the video signal wakes
    /// the thread, so it waits for one without polling. Changes are told by
    /// the signal's sequence, read before each step, so one that comes
    /// during a step is seen at the next.
    fn capture(&mut self) -> Result<()> {
        self.video.wake(self.worker.wakeup());
        let period = Duration::from_secs(1) / self.settings.fps.max(1);
        // `None`: the signal at the start counts as a change.
        let mut seen: Option<Readiness> = None;
        let mut generation = self.video.current().generation;
        let mut next = Instant::now();
        while !self.worker.stopped() {
            let readiness = self.video.current();
            if seen.is_none_or(|seen| seen.sequence != readiness.sequence) {
                self.signal_changed(seen.replace(readiness), readiness);
            }
            let kind = self.wanted(readiness.video);
            // A change of tap kind or video context starts a new period: the
            // next step opens the kind that matches, or initializes a kept
            // created-only driver again. A created-only driver holds no
            // video context and its release can hang, so only a change of
            // kind (video gone) releases it.
            if kind != self.kind || readiness.generation != generation {
                if kind != self.kind {
                    self.release_tap()?;
                    if let Some((initializations, elapsed_ms)) = self.end_refusal(Instant::now(), 0)
                    {
                        tracing::debug!(
                            initializations,
                            elapsed_ms,
                            "the refusal ended with the video"
                        );
                    }
                } else if !self.created_only() {
                    self.release_tap()?;
                }
                generation = readiness.generation;
                self.kind = kind;
                // A new period: its preparation and first failure are new.
                self.not_ready_since = None;
                self.pending_since = None;
                self.video_failing = false;
                self.retry_at = Instant::now();
            }
            let now = Instant::now();
            let waiting = self.refusal.as_ref().is_some_and(|refusal| refusal.waiting);
            let Some(kind) = kind.filter(|_| !waiting && now >= self.retry_at) else {
                // Idle or refused: until the video signal changes; retrying:
                // until the retry time, or a change first.
                if kind.is_none() || waiting {
                    self.worker.idle();
                } else {
                    self.worker
                        .sleep(self.retry_at.saturating_duration_since(now));
                }
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

    /// The video signal changed from `before` (`None`: the start) to `now`.
    /// Any change, a permission wake included, lets a refused driver be
    /// initialized once more; only a change of video or context lets a
    /// failed TV UI be tried again.
    fn signal_changed(&mut self, before: Option<Readiness>, now: Readiness) {
        self.changed_at = Instant::now();
        let Readiness {
            video,
            generation,
            sequence,
        } = now;
        tracing::debug!(?video, generation, sequence, "video signal changed");
        let video_changed = before.is_none_or(|before| before.video != video);
        if video_changed {
            tracing::info!(?video, "video changed");
        }
        if video_changed || before.is_some_and(|before| before.generation != generation) {
            self.ui = self.ui.after_change();
        }
        if let Some(refusal) = &mut self.refusal {
            refusal.waiting = false;
        }
    }

    /// The tap for this video state: video while it is available or
    /// unknown, the TV UI without video (if enabled and supported), else none.
    fn wanted(&self, video: Video) -> Option<Kind> {
        wanted(video, self.settings.include_ui, self.ui)
    }

    /// The held tap's driver is created and not initialized, as it says.
    fn created_only(&self) -> bool {
        self.tap.as_ref().is_some_and(|(_, tap)| tap.created_only())
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
                    Ok(tap) => &mut self.tap.insert((kind, tap)).1,
                    Err(fault) => return self.recover(kind, fault),
                }
            }
        };
        let initializing = tap.created_only();
        let operation = match kind {
            Kind::Video => "acquire",
            Kind::Ui => "UI acquire",
        };
        let acquired = worker.watch(operation, || tap.acquire());
        self.acquired(kind, initializing, acquired)
    }

    /// Act on one acquisition, an `initializing` one if the driver was only
    /// created: publish a frame, time a pending or preparing driver, handle
    /// a refusal, or recover from a fault.
    fn acquired(
        &mut self,
        kind: Kind,
        initializing: bool,
        acquired: Result<Acquired, Fault>,
    ) -> Result<()> {
        let now = Instant::now();
        let answer = Answer::of(&acquired);
        if initializing {
            tracing::debug!(
                ?answer,
                since_change_ms = self.since_change(now),
                "webOS video initialization answered"
            );
        }
        match answer {
            Answer::Refused => self.refused(now),
            Answer::NotReady => {
                if let Some(refusal) = &mut self.refusal {
                    refusal.initializations += 1;
                }
            }
            Answer::Accepted => {
                if let Some((initializations, elapsed_ms)) = self.end_refusal(now, 1) {
                    tracing::info!(
                        initializations,
                        elapsed_ms,
                        "the TV lets Prysm capture this video now"
                    );
                }
            }
            Answer::Failed => {
                if let Some((initializations, elapsed_ms)) = self.end_refusal(now, 1) {
                    tracing::debug!(
                        initializations,
                        elapsed_ms,
                        "the refusal ended with a native failure"
                    );
                }
            }
        }
        match acquired {
            Ok(Acquired::Frame(frame)) => {
                self.not_ready_since = None;
                self.pending_since = None;
                if kind == Kind::Video {
                    self.video_failing = false;
                }
                // The first UI frame is reported once.
                if kind == Kind::Ui && self.ui != UiSupport::Works {
                    self.ui = UiSupport::Works;
                    self.ui_failing = false;
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
                    let stalled = Fault::recoverable(Error::other(format!(
                        "webOS video produced no frame for {:?}",
                        self.limits.pending
                    )));
                    return self.recover_after(kind, stalled, Reopen::AtOnce);
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
            Ok(Acquired::Refused) => {
                // Not preparation: no deadline runs while the TV refuses.
                self.not_ready_since = None;
                self.pending_since = None;
            }
            Err(fault) => return self.recover(kind, fault),
        }
        Ok(())
    }

    /// The driver refused to initialize at `now`: it is kept, created, and
    /// initialized again at the next change of the video signal. The first
    /// refusal of a period starts it and tells the pulls.
    fn refused(&mut self, now: Instant) {
        if let Some(refusal) = &mut self.refusal {
            refusal.initializations += 1;
            refusal.waiting = true;
            return;
        }
        tracing::info!(
            initializations = 1,
            since_change_ms = self.since_change(now),
            "the TV doesn’t let Prysm capture this video; \
             initialized again at the next change of the video signal"
        );
        self.refusal = Some(Refusal {
            since: now,
            initializations: 1,
            waiting: true,
        });
        self.worker.refused(true);
    }

    /// Ends the refusal period, if one runs, and tells the pulls: its
    /// initializations (`last` more for the acquisition that ended it) and
    /// its whole milliseconds up to `now`.
    fn end_refusal(&mut self, now: Instant, last: u32) -> Option<(u32, u128)> {
        let refusal = self.refusal.take()?;
        self.worker.refused(false);
        Some((
            refusal.initializations + last,
            now.saturating_duration_since(refusal.since).as_millis(),
        ))
    }

    /// Whole milliseconds from the last change of the video signal to `now`.
    fn since_change(&self, now: Instant) -> u128 {
        now.saturating_duration_since(self.changed_at).as_millis()
    }

    /// Fatal video faults end the source; others release the tap and retry
    /// later. UI faults never end the source: lighting idles without video,
    /// as with `include_ui` off, until the video signal changes (a fault
    /// that may pass) or the next source (one that cannot). The first UI
    /// fault since a UI frame is a warning, the rest debug.
    fn recover(&mut self, kind: Kind, fault: Fault) -> Result<()> {
        self.recover_after(kind, fault, Reopen::AfterRetry)
    }

    /// [`Self::recover`], with a video tap reopened as `reopen` says.
    fn recover_after(&mut self, kind: Kind, fault: Fault, reopen: Reopen) -> Result<()> {
        let wait = match kind {
            Kind::Video => {
                let Some(wait) = video_wait(fault.recovery, reopen, &self.limits) else {
                    return Err(fault.error);
                };
                Self::retrying(&fault.error, !self.video_failing, wait);
                self.video_failing = true;
                wait
            }
            Kind::Ui => {
                self.ui = UiSupport::after_fault(fault.recovery);
                let error = chain(&fault.error);
                let message = match self.ui {
                    UiSupport::Unsupported => {
                        "TV UI capture is unsupported; no frames while there is no video"
                    }
                    _ => "TV UI capture failed; tried again once the video signal changes",
                };
                if std::mem::replace(&mut self.ui_failing, true) {
                    tracing::debug!(%error, "{message}");
                } else {
                    tracing::warn!(%error, "{message}");
                }
                self.limits.retry
            }
        };
        self.release_tap()?;
        // From after the release, which can take until the watchdog.
        self.retry_at = Instant::now() + wait;
        Ok(())
    }

    /// Logs that video capture failed with `error` and is retried in `wait`:
    /// the `first` failure of a run as a warning, the rest at debug, since
    /// only the first is news.
    fn retrying(error: &Error, first: bool, wait: Duration) {
        let error = chain(error);
        if first {
            tracing::warn!(%error, retry_in = ?wait, "webOS video capture failed; retrying");
        } else {
            tracing::debug!(%error, retry_in = ?wait, "webOS video capture failed; retrying");
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

    const LIMITS: Limits = Limits::TV;

    /// Every video fault's recovery meets every reopen: a fatal fault ends
    /// the source, a stall reopens at once, any other after `retry`.
    #[test]
    fn every_video_fault_meets_every_reopen() {
        for (recovery, reopen, wait) in [
            (Recovery::Never, Reopen::AtOnce, None),
            (Recovery::Never, Reopen::AfterRetry, None),
            (Recovery::Reopen, Reopen::AtOnce, Some(Duration::ZERO)),
            (Recovery::Reopen, Reopen::AfterRetry, Some(LIMITS.retry)),
        ] {
            assert_eq!(
                video_wait(recovery, reopen, &LIMITS),
                wait,
                "{recovery:?} x {reopen:?}"
            );
        }
    }

    /// Every UI state meets every event: a fault that cannot heal is final,
    /// one that may pass waits for a change of the video signal.
    #[test]
    fn every_ui_state_meets_every_event() {
        use UiSupport::{Failed, Unsupported, Untried, Works};
        for (state, never, reopen, change) in [
            (Untried, Unsupported, Failed, Untried),
            (Works, Unsupported, Failed, Works),
            (Failed, Unsupported, Failed, Untried),
            (Unsupported, Unsupported, Failed, Unsupported),
        ] {
            assert_eq!(UiSupport::after_fault(Recovery::Never), never, "{state:?}");
            assert_eq!(
                UiSupport::after_fault(Recovery::Reopen),
                reopen,
                "{state:?}"
            );
            assert_eq!(state.after_change(), change, "{state:?}");
        }
    }

    /// A failed or unsupported UI is not opened; video always is.
    #[test]
    fn the_tap_follows_the_video_the_setting_and_the_ui_state() {
        use UiSupport::{Failed, Unsupported, Untried, Works};
        for ui in [Untried, Works, Failed, Unsupported] {
            for include_ui in [false, true] {
                for video in [Video::Available, Video::Unknown] {
                    assert_eq!(wanted(video, include_ui, ui), Some(Kind::Video));
                }
                let ui_tap = (include_ui && matches!(ui, Untried | Works)).then_some(Kind::Ui);
                assert_eq!(
                    wanted(Video::Unavailable, include_ui, ui),
                    ui_tap,
                    "{ui:?}, include_ui {include_ui}"
                );
            }
        }
    }
}
