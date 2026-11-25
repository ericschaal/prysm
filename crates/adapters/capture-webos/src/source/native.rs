//! The native thread: owns the backend and its tap, follows the video signal,
//! paces acquisition, recovers from faults and publishes through its
//! [`Worker`].
//!
//! Every decision is [`phase::next`]'s; this shell reads the clock and the
//! signal, tells the machine what happened and performs what it asks.
use super::phase::{self, Action, Ask, Event, Kind, Note, Occurrence, Rules, State};
use super::{Backend, Limits, Tap, UiTap};
use crate::classify;
use crate::{Readiness, VideoSignal, WebosSettings};
use prysm_ports::{Error, Result, Worker, chain};
use std::time::{Duration, Instant};

/// The held tap: one kind at a time.
enum Held {
    Video(Box<dyn Tap>),
    Ui(Box<dyn UiTap>),
}

pub(super) struct Native<B> {
    /// Declared before `backend`: fields drop in order, so on a panic the
    /// tap's native cleanup still runs while the backend holds the device lock.
    tap: Option<Held>,
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
            interval: Duration::from_secs(1) / settings.fps.max(1),
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
    /// native call it answers. Returns `Ok` once stopped or failed with
    /// every tap released, or the first failed release's error.
    fn capture(&mut self) -> Result<()> {
        self.video.wake(self.worker.wakeup());
        let mut state = State::new(Instant::now());
        loop {
            // A stop or a failure: the source ends after this step.
            let mut ending = self.worker.stopped();
            let mut event = if ending {
                Event::Stop
            } else {
                self.signal(&state)
            };
            loop {
                let step = phase::next(state, event, Instant::now(), &self.rules);
                state = step.state;
                for action in step.actions {
                    match action {
                        Action::Release => self.release_tap()?,
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
                event = match step.ask {
                    Some(Ask::Open(kind)) => self.open(kind),
                    Some(Ask::Acquire) => self.acquire(),
                    None => break,
                };
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
        matches!(&self.tap, Some(Held::Video(tap)) if tap.created_only())
    }

    fn open(&mut self, kind: Kind) -> Event {
        let worker = &self.worker;
        let opened = match kind {
            Kind::Video => worker
                .watch("open", || self.backend.open(&self.settings))
                .map(Held::Video)
                .map_err(Event::OpenFailed),
            Kind::Ui => worker
                .watch("UI open", || self.backend.open_ui(&self.settings))
                .map(Held::Ui)
                .map_err(Event::UiOpenFailed),
        };
        match opened {
            Ok(held) => {
                self.tap = Some(held);
                Event::Opened
            }
            Err(failed) => failed,
        }
    }

    /// One acquisition of the held tap, told with whether it initializes a
    /// created-only driver.
    fn acquire(&mut self) -> Event {
        match self
            .tap
            .as_mut()
            .expect("the machine acquires only a held tap")
        {
            Held::Video(tap) => {
                let initializing = tap.created_only();
                Event::Acquired {
                    answer: self.worker.watch("acquire", || tap.acquire()),
                    initializing,
                }
            }
            Held::Ui(tap) => Event::UiAcquired(self.worker.watch("UI acquire", || tap.acquire())),
        }
    }

    /// Release the held tap, if any. A failed release, whenever it comes,
    /// fails the source with its cause and is the thread's result: the
    /// device may still be held, so its release is unconfirmed and
    /// escalates as [`classify::unconfirmed_release`] says.
    fn release_tap(&mut self) -> Result<()> {
        let Some(held) = self.tap.take() else {
            return Ok(());
        };
        let worker = &self.worker;
        let (released, unconfirmed) = match held {
            Held::Video(tap) => (
                worker.watch("release", || tap.release()),
                "webOS video release is unconfirmed",
            ),
            Held::Ui(tap) => (
                worker.watch("UI release", || tap.release()),
                "webOS UI capture release is unconfirmed",
            ),
        };
        released
            .map_err(|error| {
                Error::adapter_escalating(unconfirmed, error, classify::unconfirmed_release())
            })
            .inspect_err(|error| self.worker.fail(error.clone()))
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
        Note::Refused {
            initializations,
            since_change_ms,
            occurrence,
        } => {
            let message = "the TV doesn’t let Prysm capture this video; \
                           initialized again at the next change of the video signal";
            match occurrence {
                Occurrence::First => tracing::info!(initializations, since_change_ms, "{message}"),
                Occurrence::Again => tracing::debug!(initializations, since_change_ms, "{message}"),
            }
        }
        Note::CaptureAllowed { elapsed_ms } => {
            tracing::info!(elapsed_ms, "the TV lets Prysm capture this video again");
        }
        Note::RefusalEndedWithVideo { elapsed_ms } => {
            tracing::debug!(elapsed_ms, "the refusal ended with the video");
        }
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
        Note::VideoRetrying { error } => tracing::warn!(
            error = %chain(&error),
            "webOS video capture failed; opening a new driver once"
        ),
        Note::Withheld { error, occurrence } => {
            let error = chain(&error);
            let message = "the TV doesn’t let Prysm capture this video; \
                           a new driver is opened at the next change of the video signal";
            match occurrence {
                Occurrence::First => tracing::info!(%error, "{message}"),
                Occurrence::Again => tracing::debug!(%error, "{message}"),
            }
        }
        Note::ChangedWhileWithheld { video, generation } => tracing::info!(
            ?video,
            generation,
            "the video signal changed while the TV withholds the video; opening a new driver"
        ),
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
