//! The native thread: owns the backend and its tap, follows the video signal,
//! paces acquisition, retries recoverable interruptions and publishes into
//! [`Shared`].
use super::{Acquired, Backend, Fault, Limits, Shared, Tap};
use crate::{Video, VideoSignal, WebosSettings};
use prysm_ports::{Error, Result, chain};
use std::sync::Arc;
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
    /// It failed before its first frame: reported once, never retried.
    Unsupported,
}

pub(super) struct Native<B> {
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
    pub(super) fn new(
        backend: B,
        settings: WebosSettings,
        video: VideoSignal,
        limits: Limits,
        shared: Arc<Shared>,
    ) -> Self {
        Self {
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
    }

    /// Capture until cancelled or failed, release the tap, drop the backend,
    /// and publish the release result as the last act.
    pub(super) fn run(mut self) {
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
