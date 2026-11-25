//! The native thread: owns the backend and its tap, follows the video signal,
//! paces acquisition, retries recoverable interruptions and publishes through
//! its [`Worker`].
//!
//! Every decision is [`phase::next`]'s; this shell reads the clock and the
//! signal, tells the machine what happened and performs what it asks.
use super::phase::{self, Action, Event, Kind, Note, Occurrence, Rules, State};
use super::{Backend, Limits, Tap};
use crate::{Readiness, VideoSignal, WebosSettings};
use prysm_ports::{Error, Result, Worker, chain};
use std::time::{Duration, Instant};

pub(super) struct Native<B> {
    /// Declared before `backend`: fields drop in order, so on a panic the
    /// tap's native cleanup still runs while the backend holds the device lock.
    tap: Option<(Kind, Box<dyn Tap>)>,
    backend: B,
    settings: WebosSettings,
    video: VideoSignal,
    rules: Rules,
    worker: Worker,
}

impl<B: Backend> Native<B> {
    pub(super) fn new(
        backend: B,
        settings: WebosSettings,
        video: VideoSignal,
        limits: Limits,
        worker: Worker,
    ) -> Self {
        let rules = Rules {
            limits,
            include_ui: settings.include_ui,
            period: Duration::from_secs(1) / settings.fps.max(1),
        };
        Self {
            tap: None,
            backend,
            settings,
            video,
            rules,
            worker,
        }
    }

    /// Capture until stopped or failed, then drop the backend; the last
    /// release's result is the thread's last act.
    pub(super) fn run(mut self) -> Result<()> {
        let released = self.capture();
        drop(self);
        released
    }

    /// Capture until stopped or failed. A change of the video signal wakes
    /// the thread, so it waits for one without polling. Changes are told by
    /// the signal's sequence, read at the start of each run of the loop, so
    /// one that comes during a step is seen at the next. Each answer is told
    /// at once, with no stop in between, and the clock is read after the
    /// native call it answers. Returns the result of the release that ends
    /// the source.
    fn capture(&mut self) -> Result<()> {
        self.video.wake(self.worker.wakeup());
        let mut state = State::new(Instant::now());
        loop {
            // A stop or a failure: the next release, if any, ends the source.
            let mut ending = self.worker.stopped();
            let mut event = Some(if ending {
                Event::Stop
            } else {
                self.signal(&state)
            });
            while let Some(told) = event.take() {
                let step = phase::next(state, told, Instant::now(), &self.rules);
                state = step.state;
                for action in step.actions {
                    match action {
                        Action::Open(kind) => event = Some(self.open(kind)),
                        Action::Acquire => event = Some(self.acquire()),
                        Action::Release => {
                            let released = self.release_tap();
                            if ending {
                                return released;
                            }
                            // Nothing is held after a failed release.
                            if let Err(error) = released {
                                self.worker.fail(error);
                                return Ok(());
                            }
                            // Only the release after a fault is answered.
                            if state.releasing() {
                                event = Some(Event::Released);
                            }
                        }
                        Action::Deliver(frame) => self.worker.offer(frame),
                        Action::Refused(refused) => self.worker.refused(refused),
                        Action::Note(note) => log(note),
                        Action::Sleep(wait) => self.worker.sleep(wait),
                        Action::Idle => self.worker.idle(),
                        // Published before the release, which can block.
                        Action::Fail(error) => {
                            self.worker.fail(error);
                            ending = true;
                        }
                    }
                }
            }
            if ending {
                return Ok(());
            }
        }
    }

    /// The signal as an event: a change when its sequence is new.
    fn signal(&self, state: &State) -> Event {
        let readiness = self.video.current();
        if state.sequence() == Some(readiness.sequence) {
            Event::Woke
        } else {
            Event::Signal {
                readiness,
                created_only: self.created_only(),
            }
        }
    }

    /// The held tap's driver is created and not initialized, as it says.
    fn created_only(&self) -> bool {
        self.tap.as_ref().is_some_and(|(_, tap)| tap.created_only())
    }

    fn open(&mut self, kind: Kind) -> Event {
        let worker = &self.worker;
        let opened = match kind {
            Kind::Video => worker.watch("open", || self.backend.open(&self.settings)),
            Kind::Ui => worker.watch("UI open", || self.backend.open_ui(&self.settings)),
        };
        match opened {
            Ok(tap) => {
                self.tap = Some((kind, tap));
                Event::Opened
            }
            Err(fault) => Event::OpenFailed(fault),
        }
    }

    /// One acquisition of the held tap, told with whether it initializes a
    /// created-only driver.
    fn acquire(&mut self) -> Event {
        let (kind, tap) = self
            .tap
            .as_mut()
            .expect("the machine acquires only a held tap");
        let initializing = tap.created_only();
        let operation = match kind {
            Kind::Video => "acquire",
            Kind::Ui => "UI acquire",
        };
        let answer = self.worker.watch(operation, || tap.acquire());
        Event::Acquired {
            answer,
            initializing,
        }
    }

    fn release_tap(&mut self) -> Result<()> {
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

/// Writes `note` as its log line.
fn log(note: Note) {
    match note {
        Note::SignalChanged(Readiness {
            video,
            generation,
            sequence,
        }) => tracing::debug!(?video, generation, sequence, "video signal changed"),
        Note::VideoChanged(video) => tracing::info!(?video, "video changed"),
        Note::ChangedWhileRefused { video, generation } => tracing::info!(
            ?video,
            generation,
            "the video signal changed while the TV refuses; initializing the driver again"
        ),
        Note::InitializationAnswered {
            answer,
            since_change_ms,
        } => tracing::debug!(
            ?answer,
            since_change_ms,
            "webOS video initialization answered"
        ),
        Note::Refused { since_change_ms } => tracing::info!(
            initializations = 1,
            since_change_ms,
            "the TV doesn’t let Prysm capture this video; \
             initialized again at the next change of the video signal"
        ),
        Note::RefusedAgain {
            initializations,
            since_change_ms,
        } => tracing::info!(initializations, since_change_ms, "the TV refused again"),
        Note::RefusalEndedWithVideo {
            initializations,
            elapsed_ms,
        } => tracing::debug!(
            initializations,
            elapsed_ms,
            "the refusal ended with the video"
        ),
        Note::RefusalEndedPreparing {
            initializations,
            elapsed_ms,
        } => tracing::debug!(
            initializations,
            elapsed_ms,
            "the refusal ended with a driver that is preparing"
        ),
        Note::CaptureAllowed {
            initializations,
            elapsed_ms,
        } => tracing::info!(
            initializations,
            elapsed_ms,
            "the TV lets Prysm capture this video now"
        ),
        Note::RefusalEndedFailure {
            initializations,
            elapsed_ms,
        } => tracing::debug!(
            initializations,
            elapsed_ms,
            "the refusal ended with a native failure"
        ),
        Note::UiCapturing { width, height } => {
            tracing::info!(width, height, "capturing the TV UI while there is no video")
        }
        Note::UiFailed { error, occurrence } => ui_fault(
            &error,
            occurrence,
            "TV UI capture failed; tried again once the video signal changes",
        ),
        Note::UiUnsupported { error, occurrence } => ui_fault(
            &error,
            occurrence,
            "TV UI capture is unsupported; no frames while there is no video",
        ),
        Note::VideoRetrying {
            error,
            retry_in,
            occurrence,
        } => {
            let error = chain(&error);
            match occurrence {
                Occurrence::First => {
                    tracing::warn!(%error, ?retry_in, "webOS video capture failed; retrying");
                }
                Occurrence::Again => {
                    tracing::debug!(%error, ?retry_in, "webOS video capture failed; retrying");
                }
            }
        }
    }
}

/// A TV UI fault: the first since a UI frame is news, the rest debug.
fn ui_fault(error: &Error, occurrence: Occurrence, message: &str) {
    let error = chain(error);
    match occurrence {
        Occurrence::First => tracing::warn!(%error, "{message}"),
        Occurrence::Again => tracing::debug!(%error, "{message}"),
    }
}
