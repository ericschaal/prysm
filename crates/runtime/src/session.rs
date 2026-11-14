//! One session: a thread owning a source, the processor and an output, and
//! the engine's handle on it.
use crate::feeds::Feeds;
use crate::leds::{Leds, OpenOutput};
use crate::stats::{FrameTimes, Stats};
use crate::worker::{Flags, Pumped, Worker};
use prysm_core::{Config, EdgeColors, Frame};
use prysm_ports::{
    DeviceBrightness, Error, FrameSource, IdleMode, LatestSlot, OutputIssue, Result, Section,
    SessionPhase,
};
use prysm_processor::PrysmProcessor;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// Shared by the engine's [`Session`] handle and the session thread. The
/// thread reads one atomic per frame and locks only when something changed.
struct Control {
    config: LatestSlot<Config>,
    /// A config was offered and not yet taken.
    config_offered: AtomicBool,
    idle: Mutex<IdleMode>,
    /// What the held output last learned about its device; read by the
    /// engine about once a second.
    observed: Mutex<Observed>,
}

/// The held output's latest [`LedOutput::observe`](prysm_ports::LedOutput::observe)
/// and [`LedOutput::brightness`](prysm_ports::LedOutput::brightness).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Observed {
    pub issue: Option<OutputIssue>,
    pub brightness: Option<DeviceBrightness>,
}

impl Control {
    fn offer_config(&self, config: Config) {
        self.config.offer(config);
        self.config_offered.store(true, Ordering::Release);
    }

    /// The config offered since the last call, if any.
    fn take_config(&self) -> Option<Config> {
        if !self.config_offered.load(Ordering::Relaxed) {
            return None;
        }
        // Cleared before taking: an offer racing this call is either taken
        // now or flagged for the next frame.
        self.config_offered.swap(false, Ordering::AcqRel);
        self.config.take()
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
    /// When the frame before the current one was done.
    previous: Option<Instant>,
    open_output: OpenOutput,
    control: Arc<Control>,
    /// What [`Control::observed`] holds, so it is written only on a change.
    reported: Observed,
    feeds: Feeds,
    stats: Arc<Stats>,
}

impl SessionLoop {
    /// Publish what the output knows about its device, if that changed.
    fn report(&mut self, observed: Observed) {
        if observed != self.reported {
            self.reported = observed;
            *self
                .control
                .observed
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = observed;
        }
    }

    fn on_frame(&mut self, frame: Frame) -> Result<()> {
        let received = Instant::now();
        let captured = frame.captured();
        self.last_frame = received;
        if let Some(config) = self.control.take_config() {
            self.processor.configure(&config);
        }
        let shown = self.feeds.preview_frame(&frame);
        let colors = self.processor.process_frame_at(frame, received);
        let processed = Instant::now();
        let write = self
            .leds
            .write(&colors, processed, &mut self.open_output, &self.stats);
        // Recorded whether or not the write worked.
        self.stats.frame(FrameTimes {
            previous: self.previous.replace(Instant::now()),
            received,
            captured,
            processed,
            written: write.as_ref().ok().copied(),
        });
        let written = write.map_err(lights)?;
        let observed = self.leds.observe();
        self.report(observed);
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
            self.report(Observed::default());
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
            let observed = self.leds.observe();
            self.report(observed);
        }
        Ok(())
    }

    fn finish(mut self, pumped: Pumped, source: Box<dyn FrameSource>, flags: &Flags) -> SessionEnd {
        let ended = matches!(pumped, Pumped::Ended);
        let failure = pumped.failure();
        self.report(Observed::default());
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
            config_offered: AtomicBool::new(false),
            idle: Mutex::new(IdleMode::Off),
            observed: Mutex::default(),
        });
        let session = SessionLoop {
            processor: PrysmProcessor::new(&config),
            leds: Leds::Untouched,
            last: None,
            last_frame: Instant::now(),
            previous: None,
            open_output,
            control: control.clone(),
            reported: Observed::default(),
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
        self.control.offer_config(config);
    }

    /// What the held output last learned about its device: why its LEDs are
    /// not showing the session's colours, and its own brightness.
    #[must_use]
    pub fn observed(&self) -> Observed {
        *self
            .control
            .observed
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
