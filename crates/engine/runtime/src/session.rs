//! One session: a thread owning a source, the processor and an output, and
//! the engine's handle on it.
use crate::feeds::Feeds;
use crate::leds::{Configured, Device, Leds, Write};
use crate::slot::LatestSlot;
use crate::stats::{FrameTimes, Stats};
use crate::worker::{Exit, Flags, Worker, guarded};
use prysm_core::{Config, EdgeColors, Frame};
use prysm_ports::{
    DeviceReport, Error, FrameSource, Hangup, IdleMode, Pull, Result, Section, SessionPhase,
};
use prysm_processor::PrysmProcessor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// A quiet source must not let the device time out of live mode: the last
/// colors are rewritten at this interval, a fifth of WLED's default realtime
/// timeout (2.5 s, its "Realtime timeout" setting, which a user can lower).
const REFRESH: Duration = Duration::from_millis(500);

/// How long a source may deliver no frames before the session releases the
/// output to [`RELEASE`] (no signal, screen saver, no sender). The output is
/// opened only at the first frame and again at the first frame after a
/// release.
pub(crate) const VIDEO_GRACE: Duration = Duration::from_secs(3);

/// What a released output shows while waiting for frames: the device's own
/// state (WLED's preset or effect).
pub(crate) const RELEASE: IdleMode = IdleMode::Native;

/// What a session thread leaves the output in when it ends before any stop
/// asked for a mode: `Off`, the state lighting started from.
const UNASKED: IdleMode = IdleMode::Off;

/// How a session ended. Release and idle run on every exit path.
pub(crate) struct SessionEnd {
    /// Why the loop stopped.
    pub(crate) exit: Exit,
    pub(crate) released: Result<()>,
    /// The mode the output was left in: the stop's once one was asked
    /// (also when the loop then ended on its own), else [`UNASKED`]; or why
    /// idling it failed.
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
    /// The thread had ended on its own first. If it idled before the stop
    /// asked, it left the output in [`UNASKED`], and the stop then put it
    /// in its own mode through the session's output, unless the source's
    /// release is unconfirmed: `end.left` is what the output was left in
    /// last, or why that idle failed or did not return in time
    /// ([`Error::Faulted`] with [`SessionPhase::IdlingOutput`]).
    OnItsOwn(SessionEnd),
}

/// What the session thread hands back: how it ended, and its hold on the
/// output for a stop that finds it ended in another mode.
struct Finished {
    end: SessionEnd,
    /// The mode the thread idled the output in, or tried to.
    idled_in: IdleMode,
    leds: Leds,
    output: Configured,
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
    /// What the held output last learned about its device, or that the
    /// lights do not answer; read by the
    /// engine about once a second.
    observed: Mutex<DeviceReport>,
}

/// Opening, writing or idling the output is the lights' failure.
fn lights(error: Error) -> Error {
    error.in_section(Section::Output)
}

/// Leave the output in `mode`, a panic reported as the failure.
fn idle(leds: &mut Leds, mode: IdleMode, output: &Configured) -> Result<IdleMode> {
    guarded("idling the output", || leds.finish(mode, output))
        .map(|()| mode)
        .map_err(lights)
}

/// The session thread's state between frames.
///
/// Each frame: apply a pending config, process, write (opening the output if
/// not held), record the frame's times, take the output's report, publish
/// what is watched. A quiet tick rewrites the last colors (and takes the
/// report), and after [`VIDEO_GRACE`] without frames ends the quiet spell
/// as [`Leds::quiet`] says (a held output idled in [`RELEASE`] and closed,
/// a wait for the lights ended), clears the published colors and report,
/// until the next frame. While the lights do not answer an open, frames are
/// not written, the report says they are unreachable, and the open is tried
/// again as [`Leds::write`] says. The open and that idle end when a stop comes (it
/// cuts the thread's [`Hangup`]), and are then not failures: the loop sees
/// the stop.
/// When the loop ends (stop, failure, or
/// [`Pull::Ended`](prysm_ports::Pull::Ended)) the feeds are cleared,
/// `source.close()`, and the output is left in the stop's idle mode, or
/// [`UNASKED`] when no stop asked: idled if held, else put in that mode
/// without being opened, only if it was left in another one (never touched
/// for `Off` if no frame came).
struct SessionLoop {
    processor: PrysmProcessor,
    leds: Leds,
    /// The last colors written and when.
    last: Option<(EdgeColors, Instant)>,
    last_frame: Instant,
    /// When the last frame was done; `None` before the first.
    previous_done: Option<Instant>,
    output: Configured,
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

    fn on_frame(&mut self, frame: Frame, stop: &Hangup) -> Result<()> {
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
            .write(&colors, processed, &self.output, &self.stats, stop);
        // Recorded whether or not the colors were written.
        self.stats.frame(FrameTimes {
            previous_done: self.previous_done.replace(Instant::now()),
            received,
            captured,
            processed,
            written: write.as_ref().ok().copied().and_then(Write::written),
        });
        let write = write.map_err(lights)?;
        if write == Write::Stopped {
            return Ok(());
        }
        // Also while the lights do not answer: the report says so.
        let observed = self.leds.report();
        self.report(observed);
        if let Write::Written(written) = write {
            self.feeds
                .publish(&colors, shown, self.processor.viewport());
            self.last = Some((colors, written));
        }
        Ok(())
    }

    /// No frame within a tick; `refused`: the source said its device refuses
    /// the video. The quiet spell ends alike either way ([`Leds::quiet`]).
    fn on_quiet(&mut self, refused: bool, stop: &Hangup) -> Result<()> {
        self.stats.no_frame(refused);
        if self.leds.is_closed() {
            return Ok(());
        }
        let now = Instant::now();
        if now - self.last_frame >= VIDEO_GRACE {
            self.last = None;
            // The session's own state says what happens now: no lights
            // line, also after a wait for the lights.
            self.report(DeviceReport::default());
            // The device shows its own state now, not these colors.
            self.feeds.clear_colors();
            return self.leds.quiet(RELEASE, stop).map_err(lights);
        }
        if self.leds.is_held()
            && let Some((colors, written)) = &mut self.last
            && now - *written >= REFRESH
        {
            // Held: nothing is opened, so nothing for the stop to end.
            if let Write::Written(rewritten) = self
                .leds
                .write(colors, now, &self.output, &self.stats, stop)
                .map_err(lights)?
            {
                *written = rewritten;
            }
            let observed = self.leds.report();
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
        // The stop's mode once one was asked, also when the loop then ended
        // on its own: the stop then finds it idled as it asked. Unset when no
        // stop came yet, or the handle was dropped without one.
        let mode = self.control.stop_mode.get().copied().unwrap_or(UNASKED);
        let left = idle(&mut self.leds, mode, &self.output);
        Finished {
            end: SessionEnd {
                exit,
                released,
                left,
            },
            idled_in: mode,
            leds: self.leds,
            output: self.output,
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
        output: Configured,
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
            leds: Leds::Closed(Device::Untouched),
            last: None,
            last_frame: Instant::now(),
            previous_done: None,
            output,
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
                let exit = flags.pump(&mut *source, |pull| match pull {
                    Pull::Frame(frame) => session.on_frame(frame, flags.stopping()),
                    quiet => session.on_quiet(matches!(quiet, Pull::Refused), flags.stopping()),
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
    /// not showing the session's colours (also that the lights do not
    /// answer an open), and its own brightness.
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
    /// that had ended on its own and idled before this stop asked left the
    /// output in [`UNASKED`]: unless that is `mode` or its source's release
    /// is unconfirmed, the output is then put in `mode` through the
    /// session's own output, on a blocking thread and within the same
    /// deadline, and the end's `left` says how that went.
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
            idled_in,
            mut leds,
            output,
        } = self.worker.stop(deadline).await.map_err(Error::Faulted)??;
        if matches!(end.exit, Exit::Stopped) {
            return Ok(Stop::AsAsked(end));
        }
        // Idled in `mode` already (the thread read it before idling; a
        // failure there is not tried twice), or a session whose source may
        // still hold its device, which is not trusted with the output in
        // another mode.
        if idled_in == mode || end.released.is_err() {
            return Ok(Stop::OnItsOwn(end));
        }
        // The stop's idle replaces the thread's; its failure is said here.
        if let Err(error) = &end.left {
            tracing::warn!(
                error = %prysm_ports::chain(error),
                "the output's idle after the session's own end failed; idling again as the stop asks"
            );
        }
        let idling = tokio::task::spawn_blocking(move || idle(&mut leds, mode, &output));
        let left = match tokio::time::timeout_at(until, idling).await {
            // `idle` catches a panic, so the task always returns.
            Ok(left) => left.unwrap_or(Err(Error::Panicked("idling the output"))),
            Err(_) => Err(Error::Faulted(SessionPhase::IdlingOutput)),
        };
        Ok(Stop::OnItsOwn(SessionEnd { left, ..end }))
    }

    /// Join after [`Session::finished`] returned.
    pub async fn join(self) {
        self.worker.join().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::{
        AtStop, CallLog, FakeIdle, FakeOpen, FakeOutputSettings, FakeOutputs, FakeSource,
        FakeSourceSettings,
    };
    use prysm_ports::{AdapterSettings, OutputFactory, OutputIssue, Processing, Registry};
    use prysm_testkit::within_async;

    const DEADLINE: Duration = Duration::from_secs(2);

    /// A session of source `s` and output `o`.
    fn started(log: &CallLog, source: FakeSourceSettings, output: FakeOutputSettings) -> Session {
        let source = Box::new(FakeSource::new(
            log,
            FakeSourceSettings {
                name: "s".into(),
                ..source
            },
        ));
        let mut registry = Registry::default();
        registry.add_output(FakeOutputs(log.clone()));
        let output = FakeOutputSettings {
            name: "o".into(),
            ..output
        };
        let output = Configured::new(registry, AdapterSettings::new(FakeOutputs::KIND, &output));
        Session::start(
            source,
            output,
            Processing::default().config(),
            Feeds::default(),
            Arc::default(),
        )
        .map_err(|(error, _)| error)
        .unwrap()
    }

    /// A session whose source fails after two frames, once its thread has
    /// idled the output on its own.
    async fn ended_on_its_own(
        log: &CallLog,
        source: FakeSourceSettings,
        output: FakeOutputSettings,
    ) -> Session {
        let failing = FakeSourceSettings {
            fail_after: Some(2),
            ..source
        };
        let session = started(log, failing, output);
        assert!(log.wait_for("output.idle:o:Off", 1).await);
        session
    }

    /// A stop deadline no stop in these tests comes near: one that ended
    /// by it would fail their bound, [`ENDS_AT_ONCE`].
    const LATE: Duration = Duration::from_secs(30);
    /// The bound of a stop whose output call ends at the cut: the fakes'
    /// calls wait far longer for it.
    const ENDS_AT_ONCE: Duration = Duration::from_secs(1);

    /// A stop as asked, within [`ENDS_AT_ONCE`]; its end.
    async fn stopped_at_once(session: Session, mode: IdleMode) -> SessionEnd {
        let stopped = within_async(ENDS_AT_ONCE, session.stop(mode, LATE)).await;
        let Ok(Stop::AsAsked(end)) = stopped else {
            panic!("stopped as asked");
        };
        assert!(matches!(end.exit, Exit::Stopped), "{:?}", end.exit);
        assert!(end.released.is_ok());
        end
    }

    /// A stop that comes while the output opens ends the open at once, and
    /// the stop's own idle follows the source's close: once, through the
    /// output it held when the open finished, else without opening it, and
    /// not at all for an untouched device the stop turns off.
    #[tokio::test]
    async fn a_stop_ends_an_open_in_flight() {
        use IdleMode::{Native, Off};
        #[rustfmt::skip]
        let cases: [(AtStop, IdleMode, &[&str]); 6] = [
            (AtStop::Untouched, Off, &["output.open_stopped:o", "source.close:s"]),
            (AtStop::Untouched, Native, &["output.open_stopped:o", "source.close:s", "output.idle:o:Native"]),
            (AtStop::Touched, Off, &["output.open_stopped:o", "source.close:s", "output.idle:o:Off"]),
            (AtStop::Touched, Native, &["output.open_stopped:o", "source.close:s", "output.idle:o:Native"]),
            (AtStop::Opened, Off, &["output.open:o", "output.write:o", "source.close:s", "output.idle:o:Off"]),
            (AtStop::Opened, Native, &["output.open:o", "output.write:o", "source.close:s", "output.idle:o:Native"]),
        ];
        for (at_stop, mode, calls) in cases {
            let log = CallLog::new();
            let opening = FakeOutputSettings {
                open: FakeOpen::UntilStop(at_stop),
                ..Default::default()
            };
            let session = started(&log, FakeSourceSettings::default(), opening);
            assert!(log.wait_for("output.opening:o", 1).await);
            let end = stopped_at_once(session, mode).await;
            let case = (at_stop, mode);
            assert!(matches!(end.left, Ok(left) if left == mode), "{case:?}");
            let after = log.position("output.opening:o").unwrap() + 1;
            assert_eq!(log.events()[after..], *calls, "{case:?}");
        }
    }

    /// A stop that comes while the output is released after a quiet spell
    /// ends the release at once; the stop's own idle follows the source's
    /// close, once, without opening the output.
    #[tokio::test]
    async fn a_stop_ends_a_release_in_flight() {
        let log = CallLog::new();
        let quiet = FakeSourceSettings {
            quiet_after: Some(1),
            ..Default::default()
        };
        let releasing = FakeOutputSettings {
            release_until_stop: true,
            ..Default::default()
        };
        let session = started(&log, quiet, releasing);
        // After the grace without frames.
        assert!(
            log.wait_until(|events| events.iter().any(|e| e == "output.idle:o:Native"))
                .await
        );
        let end = stopped_at_once(session, IdleMode::Off).await;
        assert!(matches!(end.left, Ok(IdleMode::Off)));
        let after = log.position("output.idle:o:Native").unwrap() + 1;
        assert_eq!(
            log.events()[after..],
            [
                "output.idle_stopped:o",
                "source.close:s",
                "output.idle:o:Off"
            ]
        );
    }

    /// Lights that do not answer the open: the session runs on, says they
    /// are unreachable, and does not try the open again within its
    /// interval; the session's only failure is its source's.
    #[tokio::test]
    async fn unanswered_lights_are_reported_while_the_session_runs() {
        let log = CallLog::new();
        let gated = FakeSourceSettings {
            fail_after: Some(3),
            fail_at: Some("test.fail".into()),
            ..Default::default()
        };
        let silent = FakeOutputSettings {
            open: FakeOpen::Unanswered,
            ..Default::default()
        };
        let mut session = started(&log, gated, silent);
        // The source waits in its fourth pull, after three frames were done.
        assert!(log.wait_for("source.failing:s", 1).await);
        let unreachable = Some(OutputIssue::Unreachable);
        assert_eq!(session.observed().issue, unreachable);
        assert!(!session.has_ended());
        assert_eq!(log.count("output.open_unanswered:o"), 1);
        log.record("test.fail");
        let end = session.finished().await.unwrap();
        assert_eq!(end.result().unwrap_err().to_string(), "fake source failed");
        assert_eq!(session.observed(), DeviceReport::default());
        session.join().await;
    }

    /// A source quiet for longer than [`VIDEO_GRACE`] (and less than the
    /// wait's interval), then delivering one more frame before it fails at
    /// a gate the test opens.
    fn quiet_once() -> FakeSourceSettings {
        FakeSourceSettings {
            quiet_after: Some(1),
            quiet_for_ms: 3_500,
            fail_after: Some(2),
            fail_at: Some("test.fail".into()),
            ..Default::default()
        }
    }

    /// Lets the gated source of [`quiet_once`] fail: the session's only
    /// failure is its source's.
    async fn fails_by_its_source(mut session: Session, log: &CallLog) {
        assert!(!session.has_ended());
        log.record("test.fail");
        let end = session.finished().await.unwrap();
        assert_eq!(end.result().unwrap_err().to_string(), "fake source failed");
        session.join().await;
    }

    /// Lights that do not answer the release after a quiet spell, as an
    /// open they do not answer, fail nothing: the output is closed, the
    /// next frame opens it again.
    #[tokio::test]
    async fn an_unanswered_release_is_no_failure() {
        let log = CallLog::new();
        let unanswered = FakeOutputSettings {
            release_unanswered: true,
            ..Default::default()
        };
        let session = started(&log, quiet_once(), unanswered);
        assert!(log.wait_for("source.failing:s", 1).await);
        assert_eq!(log.count("output.idle:o:Native"), 1);
        assert_eq!(log.count("output.open:o"), 2);
        fails_by_its_source(session, &log).await;
    }

    /// A quiet spell ends a wait for the lights with nothing tried: the
    /// next frame tries the open at once, not at the wait's interval, and
    /// says again that the lights do not answer.
    #[tokio::test]
    async fn a_quiet_spell_ends_a_wait_for_the_lights() {
        let log = CallLog::new();
        let silent = FakeOutputSettings {
            open: FakeOpen::Unanswered,
            ..Default::default()
        };
        let session = started(&log, quiet_once(), silent);
        assert!(log.wait_for("source.failing:s", 1).await);
        assert_eq!(log.count("output.open_unanswered:o"), 2);
        assert_eq!(log.count("output.idle:o:Native"), 0, "nothing tried");
        assert_eq!(session.observed().issue, Some(OutputIssue::Unreachable));
        fails_by_its_source(session, &log).await;
    }

    /// A stop while waiting for the lights ends at once without another
    /// open; the stop's idle follows the finish rules: an untouched device
    /// stays untouched for `Off`.
    #[tokio::test]
    async fn a_stop_ends_a_wait_for_the_lights() {
        use IdleMode::{Native, Off};
        #[rustfmt::skip]
        let cases: [(IdleMode, &[&str]); 2] = [
            (Off, &["source.close:s"]),
            (Native, &["source.close:s", "output.idle:o:Native"]),
        ];
        for (mode, calls) in cases {
            let log = CallLog::new();
            let silent = FakeOutputSettings {
                open: FakeOpen::Unanswered,
                ..Default::default()
            };
            let session = started(&log, FakeSourceSettings::default(), silent);
            assert!(log.wait_for("output.open_unanswered:o", 1).await);
            let end = stopped_at_once(session, mode).await;
            assert!(matches!(end.left, Ok(left) if left == mode), "{mode:?}");
            let after = log.position("output.open_unanswered:o").unwrap() + 1;
            assert_eq!(log.events()[after..], *calls, "{mode:?}");
        }
    }

    #[tokio::test]
    async fn a_stop_after_an_own_end_puts_the_output_in_its_mode() {
        let log = CallLog::new();
        let session = ended_on_its_own(&log, Default::default(), Default::default()).await;
        let Stop::OnItsOwn(end) = session.stop(IdleMode::Native, DEADLINE).await.unwrap() else {
            panic!("it ended on its own");
        };
        assert!(matches!(end.exit, Exit::Failed(_)));
        assert!(matches!(end.left, Ok(IdleMode::Native)));
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
        let Stop::OnItsOwn(end) = session.stop(IdleMode::Native, DEADLINE).await.unwrap() else {
            panic!("it ended on its own");
        };
        assert!(end.released.is_err());
        assert!(matches!(end.left, Ok(IdleMode::Off)), "left as it was");
        assert_eq!(log.count("output.idle:o:Native"), 0);
    }

    /// A session whose source fails once a stop for `Native` was asked: the
    /// thread is inside the source's call, so it cannot see the stop before
    /// the failure. The stop is asked (its first poll), then the source fails.
    async fn asked_then_failing(log: &CallLog, output: FakeOutputSettings) -> Result<Stop> {
        let failing = FakeSourceSettings {
            fail_after: Some(2),
            fail_at: Some("test.fail".into()),
            ..Default::default()
        };
        let session = started(log, failing, output);
        assert!(log.wait_for("source.failing:s", 1).await);
        let (stopped, ()) = tokio::join!(session.stop(IdleMode::Native, DEADLINE), async {
            log.record("test.fail");
        });
        stopped
    }

    /// A write that fails once a stop was asked is the session's failure,
    /// not the stop: the cut ends only an open or a release, and a write
    /// never waits for it.
    #[tokio::test]
    async fn a_write_failing_after_a_stop_was_asked_is_reported() {
        let log = CallLog::new();
        let failing = FakeOutputSettings {
            fail_after: Some(1),
            fail_at: Some("test.fail".into()),
            ..Default::default()
        };
        let session = started(&log, FakeSourceSettings::default(), failing);
        assert!(log.wait_for("output.failing:o", 1).await);
        let (stopped, ()) = tokio::join!(session.stop(IdleMode::Off, DEADLINE), async {
            log.record("test.fail");
        });
        let Stop::OnItsOwn(end) = stopped.unwrap() else {
            panic!("it failed on its own");
        };
        let Exit::Failed(failure) = &end.exit else {
            panic!("the write failed: {:?}", end.exit);
        };
        assert_eq!(failure.to_string(), "fake output failed");
        assert_eq!(failure.section(), Some(Section::Output));
        assert!(matches!(end.left, Ok(IdleMode::Off)));
    }

    /// A session that fails on its own after a stop asked for another mode
    /// idles once, in the stop's mode: not first in `Off`, then again.
    #[tokio::test]
    async fn an_own_end_after_a_stop_was_asked_idles_once_in_its_mode() {
        let log = CallLog::new();
        let stopped = asked_then_failing(&log, FakeOutputSettings::default()).await;
        let Stop::OnItsOwn(end) = stopped.unwrap() else {
            panic!("it failed on its own");
        };
        assert!(matches!(end.exit, Exit::Failed(_)));
        assert!(matches!(end.left, Ok(IdleMode::Native)));
        assert_eq!(log.count("output.idle:o:Off"), 0);
        assert_eq!(log.count("output.idle:o:Native"), 1);
    }

    /// The same when that one idle fails: the stop says so, so nothing
    /// takes the output for `Native`.
    #[tokio::test]
    async fn a_failed_idle_in_the_asked_mode_is_reported() {
        let log = CallLog::new();
        let failing = FakeOutputSettings {
            idle: FakeIdle::Fails,
            ..Default::default()
        };
        let Stop::OnItsOwn(end) = asked_then_failing(&log, failing).await.unwrap() else {
            panic!("it failed on its own");
        };
        assert!(end.left.is_err(), "the idle in the asked mode failed");
        assert_eq!(log.count("output.idle:o:Native"), 1, "tried once");
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
        let Ok(Stop::OnItsOwn(end)) = stopped else {
            panic!("the end is kept");
        };
        assert!(matches!(
            end.left,
            Err(Error::Faulted(SessionPhase::IdlingOutput))
        ));
        assert_eq!(
            end.result().unwrap_err().to_string(),
            "fake source failed",
            "the session's own failure comes first"
        );
    }
}
