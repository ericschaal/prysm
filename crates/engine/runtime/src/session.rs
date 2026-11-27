//! The session effect owner. The engine chooses each next operation; this
//! thread keeps native handles and orders source close, output idle and join.
mod cleanup;

use self::cleanup::{CaptureCleanup, IdleDecision, SessionOutput, release_output};
use crate::feeds::Feeds;
use crate::session_lifecycle::OutputRelease;
use crate::slot::LatestSlot;
use crate::stats::{FrameTimes, Stats};
use crate::worker::{Exit, Flags, Worker, guarded};
use prysm_core::{Config, EdgeColors, Frame};
use prysm_ports::{
    Claim, ConfiguredOutput, ConfiguredSource, DeviceReport, Error, FrameSource, Hangup, IdleMode,
    LedOutput, OpenIntent, OutputDisplay, OutputObservation, OutputObservationState, OutputOpen,
    Power, Pull, Release, Result, Section, SessionPhase,
};
use prysm_processor::PrysmProcessor;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc as notifications, oneshot};

const REFRESH: Duration = Duration::from_millis(500);
const VIDEO_GRACE: Duration = Duration::from_secs(3);

pub(crate) enum Notice {
    CaptureOpened,
    OutputOpened,
    Handoff(OutputObservation),
    Observed(OutputObservation),
    Ended(Exit),
    IdleReady { capture_released: bool },
    Finished(Result<SessionEnd>),
}

pub(crate) struct SessionEnd {
    pub(crate) capture: Result<()>,
    pub(crate) mode: OutputRelease,
    pub(crate) output: Result<()>,
}

#[derive(Default)]
struct Control {
    config: LatestSlot<Config>,
    observed: Mutex<DeviceReport>,
}

/// Proofs stay with the worker that acquired the handles until it has joined.
pub(crate) struct SessionClaims {
    pub(crate) capture: Lease,
    pub(crate) output: Lease,
}

pub(crate) enum Lease {
    Unclaimed,
    Claimed(Claim),
    Releasing(Release),
}

pub(crate) struct Session {
    pub(crate) claims: SessionClaims,
    control: Arc<Control>,
    open_output: Option<oneshot::Sender<OpenIntent>>,
    idle: Option<oneshot::Sender<IdleDecision>>,
    notices: notifications::UnboundedReceiver<Notice>,
    worker: Worker<SessionEnd>,
    output_is_current: bool,
}

pub(crate) struct Start {
    pub(crate) output: Arc<dyn ConfiguredOutput>,
    pub(crate) config: Config,
    pub(crate) feeds: Feeds,
    pub(crate) stats: Arc<Stats>,
}

impl Session {
    pub(crate) fn start(
        capture: Arc<dyn ConfiguredSource>,
        claim: Claim,
        start: Start,
    ) -> Result<Self, (Error, Claim)> {
        Self::spawn(claim, start, move |stop| capture.open(stop))
    }

    pub(crate) fn spawn(
        claim: Claim,
        start: Start,
        open_capture: impl FnOnce(&Hangup) -> Result<Box<dyn FrameSource>> + Send + 'static,
    ) -> Result<Self, (Error, Claim)> {
        let control = Arc::new(Control::default());
        let (open_output, authorize_output) = oneshot::channel();
        let (idle, idle_after_capture) = oneshot::channel();
        let (notices, events) = notifications::unbounded_channel();
        let engine = EngineDecisions {
            authorize_output,
            idle_after_capture,
            notices,
        };
        let worker = match Worker::spawn("prysm-session", "session thread", start, {
            let control = control.clone();
            move |start, flags| run(start, open_capture, &control, engine, flags)
        }) {
            Ok(worker) => worker,
            Err((error, _)) => return Err((error, claim)),
        };
        Ok(Self {
            claims: SessionClaims {
                capture: Lease::Claimed(claim),
                output: Lease::Unclaimed,
            },
            control,
            open_output: Some(open_output),
            idle: Some(idle),
            notices: events,
            worker,
            output_is_current: true,
        })
    }

    pub(crate) async fn next(&mut self) -> Notice {
        match self.notices.recv().await {
            Some(notice) => notice,
            None => Notice::Finished(self.worker.finished().await),
        }
    }

    pub(crate) fn open_output(&mut self, intent: OpenIntent, claim: Claim) {
        self.claims.output = Lease::Claimed(claim);
        if let Some(send) = self.open_output.take() {
            let _ = send.send(intent);
        }
    }

    pub(crate) fn request_stop(&mut self) {
        self.worker.request_stop();
        self.open_output = None;
    }

    pub(crate) fn idle(&mut self, mode: OutputRelease, known_power: Option<Power>) {
        if let Some(send) = self.idle.take() {
            let _ = send.send(IdleDecision { mode, known_power });
        }
    }

    pub(crate) fn retire_output_observations(&mut self) {
        self.output_is_current = false;
    }

    pub(crate) fn output_is_current(&self) -> bool {
        self.output_is_current
    }

    pub(crate) fn deadline_phase(&self) -> SessionPhase {
        self.worker.phase()
    }

    pub(crate) async fn join(self) -> SessionClaims {
        self.worker.join().await;
        self.claims
    }

    pub(crate) fn configure(&self, config: Config) {
        self.control.config.offer(config);
    }

    pub(crate) fn observed(&self) -> DeviceReport {
        *self
            .control
            .observed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

struct EngineDecisions {
    authorize_output: oneshot::Receiver<OpenIntent>,
    idle_after_capture: oneshot::Receiver<IdleDecision>,
    notices: notifications::UnboundedSender<Notice>,
}

fn run(
    start: Start,
    open_capture: impl FnOnce(&Hangup) -> Result<Box<dyn FrameSource>>,
    control: &Control,
    engine: EngineDecisions,
    flags: &Flags,
) -> SessionEnd {
    let mut capture = None;
    let mut output = SessionOutput::default();
    let stop = flags.stopping();
    let exit = guarded("initializing screen sync", || {
        let source = open_capture(stop).map_err(|error| error.in_section(Section::Capture))?;
        let source = capture.insert(source);
        start.stats.source_opened();
        if stop.is_cut() || engine.notices.send(Notice::CaptureOpened).is_err() {
            return Ok(Exit::Stopped);
        }
        let Ok(intent) = engine.authorize_output.blocking_recv() else {
            return Ok(Exit::Stopped);
        };
        if stop.is_cut() {
            return Ok(Exit::Stopped);
        }

        let held = match start.output.open(intent, stop) {
            Ok(OutputOpen::Streaming(handle)) => output.hold(handle),
            Ok(OutputOpen::Handoff(observation)) => {
                let _ = engine.notices.send(Notice::Handoff(observation));
                return Ok(Exit::Stopped);
            }
            Err(failure) => return Err(output.open_failed(failure)),
        };
        start.stats.output_opened();
        if stop.is_cut() || engine.notices.send(Notice::OutputOpened).is_err() {
            return Ok(Exit::Stopped);
        }
        let mut stream = Stream::new(&start, control, &engine.notices);
        Ok(flags.pump(source.as_mut(), |pull| stream.process(pull, held, stop)))
    })
    .unwrap_or_else(Exit::Failed);

    let exit = normalize_exit(exit, stop);
    let capture = CaptureCleanup::after_stream(capture, &exit);
    let output = output.cleanup_after(&exit);
    let _ = engine.notices.send(Notice::Ended(exit));
    start.feeds.clear();
    flags.set_phase(SessionPhase::ClosingSource);
    let capture = capture.close();
    flags.set_phase(SessionPhase::IdlingOutput);
    let _ = engine.notices.send(Notice::IdleReady {
        capture_released: capture.is_ok(),
    });
    let decision = engine
        .idle_after_capture
        .blocking_recv()
        .unwrap_or(IdleDecision::PRESERVE);
    let output = output.finish(&*start.output, &decision);
    SessionEnd {
        capture,
        mode: decision.mode,
        output,
    }
}

fn normalize_exit(exit: Exit, stop: &Hangup) -> Exit {
    match exit {
        Exit::Failed(error)
            if error.cause::<HandedOff>().is_some()
                || (stop.is_cut() && error.cause::<prysm_ports::Stopping>().is_some()) =>
        {
            Exit::Stopped
        }
        exit => exit,
    }
}

#[derive(Debug)]
struct HandedOff;

impl std::fmt::Display for HandedOff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the device took over the display")
    }
}

impl std::error::Error for HandedOff {}

struct Stream<'a> {
    processor: PrysmProcessor,
    control: &'a Control,
    start: &'a Start,
    notices: &'a notifications::UnboundedSender<Notice>,
    last: Option<(EdgeColors, Instant)>,
    last_frame: Instant,
    previous_done: Option<Instant>,
    report: DeviceReport,
    observation: Option<OutputObservation>,
}

impl<'a> Stream<'a> {
    fn new(
        start: &'a Start,
        control: &'a Control,
        notices: &'a notifications::UnboundedSender<Notice>,
    ) -> Self {
        Self {
            processor: PrysmProcessor::new(&start.config),
            control,
            start,
            notices,
            last: None,
            last_frame: Instant::now(),
            previous_done: None,
            report: DeviceReport::default(),
            observation: None,
        }
    }

    fn process(
        &mut self,
        pull: Pull,
        output: &mut Option<Box<dyn LedOutput>>,
        stop: &Hangup,
    ) -> Result<()> {
        match pull {
            Pull::Frame(frame) => {
                let held = match output {
                    Some(handle) => handle,
                    None => match self
                        .start
                        .output
                        .open(OpenIntent::Resume, stop)
                        .map_err(|failure| failure.into_error().in_section(Section::Output))?
                    {
                        OutputOpen::Streaming(handle) => {
                            self.start.stats.output_opened();
                            output.insert(handle)
                        }
                        OutputOpen::Handoff(observation) => return self.handoff(observation),
                    },
                };
                self.on_frame(frame, held.as_mut())?;
            }
            quiet => {
                let now = Instant::now();
                let refused = matches!(quiet, Pull::Refused);
                if now - self.last_frame >= VIDEO_GRACE {
                    self.start.stats.no_frame(refused);
                    if let Some(held) = output.take() {
                        self.last = None;
                        self.start.feeds.clear_colors();
                        let released = release_output(
                            held,
                            prysm_ports::OutputRelease::Ordinary(IdleMode::Native),
                            stop,
                        );
                        if let Err(error) = released
                            && error.escalation() != prysm_ports::Escalation::Unanswered
                        {
                            return Err(error.in_section(Section::Output));
                        }
                    }
                } else if let Some(held) = output.as_mut() {
                    self.on_quiet(refused, &mut **held, now)?;
                }
            }
        }
        let report = output
            .as_mut()
            .map_or_else(DeviceReport::default, |output| output.report());
        if report != self.report {
            self.report = report;
            *self
                .control
                .observed
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = report;
        }
        if let Some(held) = output.as_mut()
            && let OutputObservationState::Observed(observation) = held.stream_observation()
            && self.observation != Some(observation)
        {
            self.observation = Some(observation);
            if observation.display == OutputDisplay::Override {
                return self.handoff(observation);
            }
            let _ = self.notices.send(Notice::Observed(observation));
        }
        Ok(())
    }

    fn handoff(&self, observation: OutputObservation) -> Result<()> {
        let _ = self.notices.send(Notice::Handoff(observation));
        Err(Error::adapter("output handoff", HandedOff))
    }

    fn write(
        &self,
        output: &mut dyn LedOutput,
        colors: &EdgeColors,
        began: Instant,
    ) -> Result<Instant> {
        let result = output
            .write(colors)
            .map_err(|error| error.in_section(Section::Output));
        let at = Instant::now();
        self.start.stats.output_write(at - began, result.is_ok());
        result.map(|()| at)
    }

    fn on_frame(&mut self, frame: Frame, output: &mut dyn LedOutput) -> Result<()> {
        let received = Instant::now();
        let captured = frame.captured();
        self.last_frame = received;
        if let Some(config) = self.control.config.take() {
            self.processor.configure(&config);
        }
        let preview = self.start.feeds.preview_frame(&frame);
        let colors = self.processor.process_frame_at(frame, received);
        let processed = Instant::now();
        let wrote = self.write(output, &colors, processed);
        self.start.stats.frame(FrameTimes {
            previous_done: self.previous_done.replace(Instant::now()),
            received,
            captured,
            processed,
            written: wrote.as_ref().ok().copied(),
        });
        let written = wrote?;
        self.start
            .feeds
            .publish(&colors, preview, self.processor.viewport());
        self.last = Some((colors, written));
        Ok(())
    }

    fn on_quiet(&mut self, refused: bool, output: &mut dyn LedOutput, now: Instant) -> Result<()> {
        self.start.stats.no_frame(refused);
        if let Some((colors, written)) = self.last.as_ref()
            && now - *written >= REFRESH
        {
            let at = self.write(output, colors, now)?;
            if let Some((_, written)) = &mut self.last {
                *written = at;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_loop_takes_only_the_newest_pending_config() {
        let control = Control::default();
        control.config.offer(Config::default());
        let latest = Config {
            brightness_percent: prysm_core::BrightnessPercent(27),
            ..Config::default()
        };
        control.config.offer(latest);
        assert_eq!(control.config.take(), Some(latest));
        assert!(control.config.take().is_none());
    }
}
