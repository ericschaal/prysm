//! One session: a thread owning a source, the processor and an output, and
//! the engine's handle on it.
use crate::feeds::Feeds;
use crate::leds::{Leds, OpenOutput};
use crate::slot::LatestSlot;
use crate::stats::{FrameTimes, Stats};
use crate::worker::{Exit, Flags, Worker, guarded};
use prysm_core::{Config, EdgeColors, Frame};
use prysm_ports::{DeviceReport, Error, FrameSource, IdleMode, Result, Section, SessionPhase};
use prysm_processor::PrysmProcessor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
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

/// What a session thread leaves the output in when it ends without being
/// asked to stop: `Off`, the state lighting started from.
const UNASKED: IdleMode = IdleMode::Off;

/// How a session ended. Release and idle run on every exit path.
pub(crate) struct SessionEnd {
    /// Why the loop stopped.
    pub(crate) exit: Exit,
    pub(crate) released: Result<()>,
    /// The mode the output was left in: the stop's when the thread was asked
    /// to stop, else [`UNASKED`]; or why idling it failed.
    pub(crate) left: Result<IdleMode>,
}

impl SessionEnd {
    /// The first failure, in the order it happened.
    ///
    /// # Errors
    /// The loop failure, else the release failure, else the idle failure.
    pub(crate) fn result(self) -> Result<()> {
        match self.exit {
            Exit::Failed(failure) => Err(failure),
            Exit::Stopped | Exit::Ended => self.released.and(self.left.map(drop)),
        }
    }
}

/// How [`Session::stop`] found the session.
pub(crate) enum Stop {
    /// The thread stopped because it was asked to: the output was left in
    /// the stop's mode, as `end.left` says.
    AsAsked(SessionEnd),
    /// The thread had ended on its own first and left the output in
    /// [`UNASKED`]; `idled` is putting it in the stop's mode after that,
    /// through the session's own output (skipped, `Ok`, when the source's
    /// release is unconfirmed).
    OnItsOwn { end: SessionEnd, idled: Result<()> },
}

/// What the session thread hands back: how it ended, and its hold on the
/// output for a stop that finds it ended in another mode.
struct Finished {
    end: SessionEnd,
    leds: Leds,
    open_output: OpenOutput,
}

/// Shared by the engine's [`Session`] handle and the session thread.
struct Control {
    /// Taken by the thread at each frame.
    config: LatestSlot<Config>,
    /// The mode a stop asks for: set before the thread is asked to stop, and
    /// read by it only then.
    stop_mode: OnceLock<IdleMode>,
    /// Set by the thread as soon as its loop ended on its own, before it
    /// releases anything; see [`Session::has_ended`].
    ended: AtomicBool,
    /// What the held output last learned about its device; read by the
    /// engine about once a second.
    observed: Mutex<DeviceReport>,
}

/// Opening, writing or idling the output is the lights' failure.
fn lights(error: Error) -> Error {
    error.in_section(Section::Output)
}

/// Leave the output in `mode`, a panic reported as the failure.
fn idle(leds: &mut Leds, mode: IdleMode, open: &mut OpenOutput) -> Result<IdleMode> {
    guarded("idling the output", || leds.finish(mode, open))
        .map(|()| mode)
        .map_err(lights)
}

/// The session thread's state between frames.
///
/// Each frame: apply a pending config, process, write (opening the output if
/// not held), record the frame's times, observe, publish what is watched. A quiet tick rewrites (and observes) the last colors, and after
/// [`VIDEO_GRACE`] without frames idles the output in [`RELEASE`], clears the
/// published colors and closes it until the next frame. When the loop ends
/// (stop, failure, or [`Pull::Ended`](prysm_ports::Pull::Ended)) the feeds
/// are cleared, `source.close()`, and the output is left in the stop's idle
/// mode, or [`UNASKED`] when no stop asked (idled if held, reopened only if
/// it was left in another mode; never opened for `Off` if no frame came).
struct SessionLoop {
    processor: PrysmProcessor,
    leds: Leds,
    /// The last colors written and when.
    last: Option<(EdgeColors, Instant)>,
    last_frame: Instant,
    /// When the last frame was done; `None` before the first.
    previous_done: Option<Instant>,
    open_output: OpenOutput,
    control: Arc<Control>,
    /// What [`Control::observed`] holds, so it is written only on a change.
    reported: DeviceReport,
    feeds: Feeds,
    stats: Arc<Stats>,
}

impl SessionLoop {
    /// Publish what the output knows about its device, if that changed.
    fn report(&mut self, observed: DeviceReport) {
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
        if let Some(config) = self.control.config.take() {
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
            previous_done: self.previous_done.replace(Instant::now()),
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
            self.report(DeviceReport::default());
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

    fn finish(mut self, exit: Exit, source: Box<dyn FrameSource>, flags: &Flags) -> Finished {
        if !matches!(exit, Exit::Stopped) {
            self.control.ended.store(true, Ordering::Release);
        }
        self.report(DeviceReport::default());
        self.feeds.clear();
        flags.set_phase(SessionPhase::ClosingSource);
        // A panic in either is reported as that step's failure, and the
        // output is idled after a source that panicked while closing.
        let released = guarded("closing the source", || source.close());
        flags.set_phase(SessionPhase::IdlingOutput);
        let mode = match exit {
            // Unset when the handle was dropped without a stop.
            Exit::Stopped => self.control.stop_mode.get().copied().unwrap_or(UNASKED),
            Exit::Ended | Exit::Failed(_) => UNASKED,
        };
        let left = idle(&mut self.leds, mode, &mut self.open_output);
        Finished {
            end: SessionEnd {
                exit,
                released,
                left,
            },
            leds: self.leds,
            open_output: self.open_output,
        }
    }
}

/// One dedicated thread owning a source, the processor and an output; the
/// rules are [`SessionLoop`]'s.
pub(crate) struct Session {
    control: Arc<Control>,
    worker: Worker<Finished>,
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
            stop_mode: OnceLock::new(),
            ended: AtomicBool::new(false),
            observed: Mutex::default(),
        });
        let session = SessionLoop {
            processor: PrysmProcessor::new(&config),
            leds: Leds::Untouched,
            last: None,
            last_frame: Instant::now(),
            previous_done: None,
            open_output,
            control: control.clone(),
            reported: DeviceReport::default(),
            feeds,
            stats,
        };
        let worker = Worker::spawn(
            "prysm-session",
            "session thread",
            (source, session),
            |(mut source, mut session), flags| {
                let exit = flags.pump(&mut *source, |frame| match frame {
                    Some(frame) => session.on_frame(frame),
                    None => session.on_quiet(),
                });
                session.finish(exit, source, flags)
            },
        )
        .map_err(|(error, (source, _))| (error, source))?;
        Ok(Self { control, worker })
    }

    /// Apply processing settings at the next frame boundary.
    pub fn configure(&self, config: Config) {
        self.control.config.offer(config);
    }

    /// What the held output last learned about its device: why its LEDs are
    /// not showing the session's colours, and its own brightness.
    #[must_use]
    pub fn observed(&self) -> DeviceReport {
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
        self.worker.finished().await.map(|finished| finished.end)
    }

    /// The thread's loop ended on its own (a failure or the source ending);
    /// it may still be releasing what it holds.
    pub fn has_ended(&self) -> bool {
        self.control.ended.load(Ordering::Acquire)
    }

    /// Stop, release, idle with `mode` and join within `deadline`. A thread
    /// that had ended on its own left the output in [`UNASKED`]: unless its
    /// source's release is unconfirmed, the output is then put in `mode`
    /// through the session's own output, on a blocking thread and within the
    /// same deadline; `idled` is [`Error::Faulted`] with
    /// [`SessionPhase::IdlingOutput`] if that did not return in time.
    ///
    /// # Errors
    /// [`Error::Faulted`] with the phase the thread is stuck in when the
    /// deadline passes (the thread is then abandoned), or [`Error::Panicked`].
    pub async fn stop(self, mode: IdleMode, deadline: Duration) -> Result<Stop> {
        let until = tokio::time::Instant::now() + deadline;
        // Set before the stop request: a thread that sees the request reads it.
        let _ = self.control.stop_mode.set(mode);
        let Finished {
            end,
            mut leds,
            mut open_output,
        } = self.worker.stop(deadline).await.map_err(Error::Faulted)??;
        if matches!(end.exit, Exit::Stopped) {
            return Ok(Stop::AsAsked(end));
        }
        // A session whose source may still hold its device is not trusted
        // with the output in another mode.
        if mode == UNASKED || end.released.is_err() {
            return Ok(Stop::OnItsOwn { end, idled: Ok(()) });
        }
        let idling =
            tokio::task::spawn_blocking(move || idle(&mut leds, mode, &mut open_output).map(drop));
        let idled = match tokio::time::timeout_at(until, idling).await {
            // `idle` catches a panic, so the task always returns.
            Ok(idled) => idled.unwrap_or(Err(Error::Panicked("idling the output"))),
            Err(_) => Err(Error::Faulted(SessionPhase::IdlingOutput)),
        };
        Ok(Stop::OnItsOwn { end, idled })
    }

    /// Join after [`Session::finished`] returned.
    pub async fn join(self) {
        self.worker.join().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::{CallLog, FakeOutputSettings, FakeOutputs, FakeSource, FakeSourceSettings};
    use prysm_ports::{OutputFactory, Processing};

    const DEADLINE: Duration = Duration::from_secs(2);

    /// A session whose source fails after two frames, once its thread has
    /// idled the output on its own.
    async fn ended_on_its_own(
        log: &CallLog,
        source: FakeSourceSettings,
        output: FakeOutputSettings,
    ) -> Session {
        let source = Box::new(FakeSource::new(
            log,
            FakeSourceSettings {
                name: "s".into(),
                fail_after: Some(2),
                ..source
            },
        ));
        let outputs = FakeOutputs(log.clone());
        let output = FakeOutputSettings {
            name: "o".into(),
            ..output
        };
        let open: OpenOutput = Box::new(move || outputs.open(&output));
        let session = Session::start(
            source,
            open,
            Processing::default().config(),
            Feeds::default(),
            Arc::default(),
        )
        .map_err(|(error, _)| error)
        .unwrap();
        assert!(log.wait_for("output.idle:o:Off", 1).await);
        session
    }

    #[tokio::test]
    async fn a_stop_after_an_own_end_puts_the_output_in_its_mode() {
        let log = CallLog::new();
        let session = ended_on_its_own(&log, Default::default(), Default::default()).await;
        let Stop::OnItsOwn { end, idled } = session.stop(IdleMode::Native, DEADLINE).await.unwrap()
        else {
            panic!("it ended on its own");
        };
        assert!(matches!(end.exit, Exit::Failed(_)));
        assert!(idled.is_ok());
        assert_eq!(log.count("output.idle:o:Native"), 1);
    }

    /// A source that may still hold its device: the output is not put in
    /// another mode for a session that cannot be trusted.
    #[tokio::test]
    async fn a_stop_after_an_own_end_with_an_unconfirmed_release_idles_nothing_more() {
        let log = CallLog::new();
        let unreleased = FakeSourceSettings {
            fail_close: true,
            ..Default::default()
        };
        let session = ended_on_its_own(&log, unreleased, Default::default()).await;
        let Stop::OnItsOwn { end, idled } = session.stop(IdleMode::Native, DEADLINE).await.unwrap()
        else {
            panic!("it ended on its own");
        };
        assert!(end.released.is_err());
        assert!(idled.is_ok());
        assert_eq!(log.count("output.idle:o:Native"), 0);
    }

    /// An idle in the stop's mode that does not return in time is reported
    /// as such, and the end it followed is kept with its own failure.
    #[tokio::test]
    async fn a_late_idle_after_an_own_end_keeps_the_end() {
        let log = CallLog::new();
        let slow = FakeOutputSettings {
            native_idle_delay_ms: 3_000,
            ..Default::default()
        };
        let session = ended_on_its_own(&log, Default::default(), slow).await;
        let stopped = session
            .stop(IdleMode::Native, Duration::from_millis(300))
            .await;
        let Ok(Stop::OnItsOwn { end, idled }) = stopped else {
            panic!("the end is kept");
        };
        assert!(matches!(
            idled,
            Err(Error::Faulted(SessionPhase::IdlingOutput))
        ));
        assert_eq!(
            end.result().unwrap_err().to_string(),
            "fake source failed",
            "the session's own failure comes first"
        );
    }
}
