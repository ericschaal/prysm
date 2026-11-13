//! One session: a thread owning a source, the processor and an output, and
//! the engine's handle on it.
use crate::feeds::Feeds;
use crate::leds::{Leds, OpenOutput};
use crate::stats::{FrameTimes, Stats};
use crate::worker::{Flags, Pumped, Worker};
use prysm_core::{Config, EdgeColors, Frame};
use prysm_ports::{
    Error, FrameSource, IdleMode, LatestSlot, OutputIssue, Result, Section, SessionPhase,
};
use prysm_processor::PrysmProcessor;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// A quiet source must not let the device time out of live mode: the last
/// colors are rewritten at this interval.
const REFRESH: Duration = Duration::from_millis(500);

/// How long a source may deliver no frames before the session releases the
/// output to [`RELEASE`] (no signal, screen saver, no sender). The output is
/// opened only at the first frame and again at the first frame after a
/// release.
pub(crate) const VIDEO_GRACE: Duration = Duration::from_secs(3);

/// What a released output shows while waiting for frames: the device's own
/// state (WLED's preset or effect).
pub(crate) const RELEASE: IdleMode = IdleMode::Native;

/// How a session ended. Release and idle run on every exit path.
#[derive(Debug)]
pub(crate) struct SessionEnd {
    /// Why the loop stopped; `None` when it was asked to or the source ended.
    pub(crate) failure: Option<Error>,
    /// The source reported [`Pull::Ended`](prysm_ports::Pull::Ended): the
    /// session finished on its own without a failure.
    pub(crate) ended: bool,
    pub(crate) released: Result<()>,
    pub(crate) idled: Result<()>,
}

impl SessionEnd {
    /// The first failure, in the order it happened.
    ///
    /// # Errors
    /// The loop failure, else the release failure, else the idle failure.
    pub(crate) fn result(self) -> Result<()> {
        match self.failure {
            Some(failure) => Err(failure),
            None => self.released.and(self.idled),
        }
    }
}

struct Control {
    config: LatestSlot<Config>,
    idle: Mutex<IdleMode>,
    /// The held output's latest [`LedOutput::observe`]; read by the engine
    /// about once a second.
    issue: Mutex<Option<OutputIssue>>,
}

impl Control {
    fn report(&self, issue: Option<OutputIssue>) {
        *self.issue.lock().unwrap_or_else(PoisonError::into_inner) = issue;
    }
}

/// Opening, writing or idling the output is the lights' failure.
fn lights(error: Error) -> Error {
    error.in_section(Section::Output)
}

/// The session thread's state between frames.
///
/// Each frame: apply a pending config, process, write (opening the output if
/// not held), record the frame's times, observe, publish what is watched. A quiet tick rewrites (and observes) the last colors, and after
/// [`VIDEO_GRACE`] without frames idles the output in [`RELEASE`], clears the
/// published colors and closes it until the next frame. When the loop ends
/// (stop, failure, or [`Pull::Ended`](prysm_ports::Pull::Ended)) the feeds
/// are cleared, `source.close()`, and the output is left in the
/// stop's idle mode (idled if held, reopened only if it was left in another
/// mode; never opened for `Off` if no frame came).
struct SessionLoop {
    processor: PrysmProcessor,
    leds: Leds,
    /// The last colors written and when.
    last: Option<(EdgeColors, Instant)>,
    last_frame: Instant,
    open_output: OpenOutput,
    control: Arc<Control>,
    feeds: Feeds,
    stats: Arc<Stats>,
}

impl SessionLoop {
    fn on_frame(&mut self, frame: Frame) -> Result<()> {
        let received = Instant::now();
        let captured = frame.captured();
        self.last_frame = received;
        if let Some(config) = self.control.config.take() {
            self.processor.configure(&config);
        }
        let shown = self.feeds.preview_frame(&frame);
        let colors = self.processor.process_frame_at(frame, received);
        let processed = Instant::now();
        let written = self
            .leds
            .write(&colors, processed, &mut self.open_output, &self.stats);
        self.stats.frame(FrameTimes {
            received,
            captured,
            processed,
            written: written.as_ref().ok().copied(),
        });
        let written = written.map_err(lights)?;
        self.control.report(self.leds.observe());
        self.feeds
            .publish(&colors, shown, self.processor.viewport());
        self.last = Some((colors, written));
        Ok(())
    }

    fn on_quiet(&mut self) -> Result<()> {
        if !self.leds.is_held() {
            return Ok(());
        }
        let now = Instant::now();
        if now - self.last_frame >= VIDEO_GRACE {
            self.last = None;
            self.control.report(None);
            // The device shows its own state now, not these colors.
            self.feeds.clear_colors();
            return self.leds.release(RELEASE).map_err(lights);
        }
        if let Some((colors, written)) = &mut self.last
            && now - *written >= REFRESH
        {
            *written = self
                .leds
                .write(colors, now, &mut self.open_output, &self.stats)
                .map_err(lights)?;
            self.control.report(self.leds.observe());
        }
        Ok(())
    }

    fn finish(mut self, pumped: Pumped, source: Box<dyn FrameSource>, flags: &Flags) -> SessionEnd {
        let ended = matches!(pumped, Pumped::Ended);
        let failure = pumped.failure();
        self.control.report(None);
        self.feeds.clear();
        flags.set_phase(SessionPhase::ClosingSource);
        let released = source.close();
        flags.set_phase(SessionPhase::IdlingOutput);
        let mode = *self
            .control
            .idle
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let idled = self
            .leds
            .finish(mode, &mut self.open_output)
            .map_err(lights);
        SessionEnd {
            failure,
            ended,
            released,
            idled,
        }
    }
}

/// One dedicated thread owning a source, the processor and an output; the
/// rules are [`SessionLoop`]'s.
pub(crate) struct Session {
    control: Arc<Control>,
    worker: Worker<SessionEnd>,
}

impl Session {
    /// # Errors
    /// The thread could not be spawned. The source comes back unclosed, so the
    /// caller can still release it.
    pub(crate) fn start(
        source: Box<dyn FrameSource>,
        open_output: OpenOutput,
        config: Config,
        feeds: Feeds,
        stats: Arc<Stats>,
    ) -> Result<Self, (Error, Box<dyn FrameSource>)> {
        let control = Arc::new(Control {
            config: LatestSlot::new(),
            idle: Mutex::new(IdleMode::Off),
            issue: Mutex::new(None),
        });
        let session = SessionLoop {
            processor: PrysmProcessor::new(&config),
            leds: Leds::Untouched,
            last: None,
            last_frame: Instant::now(),
            open_output,
            control: control.clone(),
            feeds,
            stats,
        };
        let worker = Worker::spawn(
            "prysm-session",
            (source, session),
            |(mut source, mut session), flags| {
                let pumped = flags.pump(&mut *source, |frame| match frame {
                    Some(frame) => session.on_frame(frame),
                    None => session.on_quiet(),
                });
                session.finish(pumped, source, flags)
            },
        )
        .map_err(|(error, (source, _))| (error, source))?;
        Ok(Self { control, worker })
    }

    /// Apply processing settings at the next frame boundary.
    pub fn configure(&self, config: Config) {
        self.control.config.offer(config);
    }

    /// Why the held output's LEDs are not showing the session's colours.
    #[must_use]
    pub fn output_issue(&self) -> Option<OutputIssue> {
        *self
            .control
            .issue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Wait for the session to end on its own (a failure or the source
    /// ending). Cancel-safe.
    ///
    /// # Errors
    /// The thread panicked while releasing its adapters.
    pub async fn finished(&mut self) -> Result<SessionEnd> {
        self.worker.finished().await
    }

    /// Stop, release, idle with `mode` and join within `deadline`.
    ///
    /// # Errors
    /// [`Error::Faulted`] with the phase the thread is stuck in when the
    /// deadline passes (the thread is then abandoned), or [`Error::Panicked`].
    pub async fn stop(self, mode: IdleMode, deadline: Duration) -> Result<SessionEnd> {
        *self
            .control
            .idle
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = mode;
        self.worker.stop(deadline).await.map_err(Error::Faulted)?
    }

    /// Join after [`Session::finished`] returned.
    pub async fn join(self) {
        self.worker.join().await;
    }
}
