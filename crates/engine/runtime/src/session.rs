//! The session effect owner. The engine chooses each next operation; this
//! thread keeps native handles and orders source close, output idle and join.
mod cleanup;

pub(crate) use self::cleanup::IdleDecision;
use self::cleanup::{Access, CaptureCleanup, SessionOutput};
use crate::feeds::Feeds;
use crate::session_lifecycle::OutputRelease;
use crate::slot::LatestSlot;
use crate::stats::{FrameTimes, Stats};
use crate::worker::{Exit, Flags, Worker, guarded};
use prysm_core::{Config, EdgeColors, Frame};
use prysm_ports::{
    Claim, ConfiguredOutput, ConfiguredSource, DeviceReport, Error, Hangup, LedOutput, OpenIntent,
    OutputDisplay, OutputObservation, OutputObservationState, Pull, Release, Result, Section,
    SessionPhase,
};
use prysm_processor::PrysmProcessor;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc as notifications, oneshot};

const REFRESH: Duration = Duration::from_millis(500);
const VIDEO_GRACE: Duration = Duration::from_secs(3);

pub(crate) enum Notice {
    CaptureOpened {
        authorize: oneshot::Sender<OpenIntent>,
    },
    OutputOpened,
    Handoff(OutputObservation),
    Observed(OutputObservation),
    Ended(Exit),
    CaptureClosed {
        released: bool,
        decide_idle: oneshot::Sender<IdleDecision>,
    },
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
    notices: notifications::UnboundedReceiver<Notice>,
    worker: Worker<SessionEnd>,
    output_is_current: bool,
}

pub(crate) struct Start {
    pub(crate) capture: Arc<dyn ConfiguredSource>,
    pub(crate) output: Arc<dyn ConfiguredOutput>,
    pub(crate) config: Config,
    pub(crate) feeds: Feeds,
    pub(crate) stats: Arc<Stats>,
}

impl Session {
    pub(crate) fn start(claim: Claim, start: Start) -> Result<Self, (Error, Claim)> {
        let control = Arc::new(Control::default());
        let (notices, events) = notifications::unbounded_channel();
        let session = SessionWorker {
            control: control.clone(),
            notices,
        };
        let worker = match Worker::spawn(
            "prysm-session",
            "session thread",
            start,
            move |start, flags| session.run(start, flags),
        ) {
            Ok(worker) => worker,
            Err((error, _)) => return Err((error, claim)),
        };
        Ok(Self {
            claims: SessionClaims {
                capture: Lease::Claimed(claim),
                output: Lease::Unclaimed,
            },
            control,
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

    pub(crate) fn request_stop(&self) {
        self.worker.request_stop();
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

struct SessionWorker {
    control: Arc<Control>,
    notices: notifications::UnboundedSender<Notice>,
}

impl SessionWorker {
    fn run(self, start: Start, flags: &Flags) -> SessionEnd {
        let stop = flags.stopping();
        let mut capture = None;
        let mut output = SessionOutput::new(&*start.output, &start.stats);

        let exit = guarded("initializing screen sync", || {
            let source = {
                let adapter = start.capture;
                adapter.open(stop)
            }
            .map_err(|error| error.in_section(Section::Capture))?;
            let source = capture.insert(source);
            start.stats.source_opened();

            let Some(intent) = self.await_output_authorization(stop) else {
                return Ok(Exit::Stopped);
            };
            if let Access::Handoff(observation) = output.open_authorized(intent, stop)? {
                let _ = self.notices.send(Notice::Handoff(observation));
                return Ok(Exit::Stopped);
            }
            if stop.is_cut() || self.notices.send(Notice::OutputOpened).is_err() {
                return Ok(Exit::Stopped);
            }
            let mut stream = Stream::new(&self, &start.config, &start.feeds, &start.stats);
            Ok(flags.pump(source.as_mut(), |pull| {
                stream.process(pull, &mut output, stop)
            }))
        })
        .unwrap_or_else(Exit::Failed);

        let exit = normalize_exit(exit, stop);
        let capture = CaptureCleanup::after_stream(capture, &exit);
        let output = output.cleanup_after(&exit);
        let _ = self.notices.send(Notice::Ended(exit));
        start.feeds.clear();
        flags.set_phase(SessionPhase::ClosingSource);
        let capture = capture.close();

        flags.set_phase(SessionPhase::IdlingOutput);
        let decision = request_idle(self.notices, capture.is_ok())
            .blocking_recv()
            .unwrap_or(IdleDecision::PRESERVE);
        SessionEnd {
            capture,
            mode: decision.mode,
            output: output.finish(&decision),
        }
    }

    fn await_output_authorization(&self, stop: &Hangup) -> Option<OpenIntent> {
        let (authorize, answer) = oneshot::channel();
        let (answer, cancel) = futures::future::abortable(answer);
        let _wake_on_stop = stop.on_cut(move || cancel.abort())?;
        self.notices
            .send(Notice::CaptureOpened { authorize })
            .ok()?;
        futures::executor::block_on(answer).ok()?.ok()
    }
}

/// Sends the final notice and relinquishes the worker's sender before waiting for a reply.
fn request_idle(
    notices: notifications::UnboundedSender<Notice>,
    released: bool,
) -> oneshot::Receiver<IdleDecision> {
    let (decide_idle, answer) = oneshot::channel();
    let _ = notices.send(Notice::CaptureClosed {
        released,
        decide_idle,
    });
    answer
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
    feeds: &'a Feeds,
    stats: &'a Stats,
    notices: &'a notifications::UnboundedSender<Notice>,
    last: Option<(EdgeColors, Instant)>,
    last_frame: Instant,
    previous_done: Option<Instant>,
    report: DeviceReport,
    observation: Option<OutputObservation>,
}

impl<'a> Stream<'a> {
    fn new(worker: &'a SessionWorker, config: &Config, feeds: &'a Feeds, stats: &'a Stats) -> Self {
        Self {
            processor: PrysmProcessor::new(config),
            control: &worker.control,
            feeds,
            stats,
            notices: &worker.notices,
            last: None,
            last_frame: Instant::now(),
            previous_done: None,
            report: DeviceReport::default(),
            observation: None,
        }
    }

    fn process(&mut self, pull: Pull, output: &mut SessionOutput, stop: &Hangup) -> Result<()> {
        match pull {
            Pull::Frame(frame) => match output.streaming(stop)? {
                Access::Streaming(held) => self.on_frame(frame, held)?,
                Access::Handoff(observation) => return self.handoff(observation),
            },
            quiet => {
                let now = Instant::now();
                let refused = matches!(quiet, Pull::Refused);
                if now - self.last_frame >= VIDEO_GRACE {
                    self.stats.no_frame(refused);
                    if output.held().is_some() {
                        self.last = None;
                        self.feeds.clear_colors();
                        output.release_quiet(stop)?;
                    }
                } else if let Some(held) = output.held() {
                    self.on_quiet(refused, held, now)?;
                }
            }
        }
        let report = output
            .held()
            .map_or_else(DeviceReport::default, |output| output.report());
        if report != self.report {
            self.report = report;
            *self
                .control
                .observed
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = report;
        }
        if let Some(held) = output.held()
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
        self.stats.output_write(at - began, result.is_ok());
        result.map(|()| at)
    }

    fn on_frame(&mut self, frame: Frame, output: &mut dyn LedOutput) -> Result<()> {
        let received = Instant::now();
        let captured = frame.captured();
        self.last_frame = received;
        if let Some(config) = self.control.config.take() {
            self.processor.configure(&config);
        }
        let preview = self.feeds.preview_frame(&frame);
        let colors = self.processor.process_frame_at(frame, received);
        let processed = Instant::now();
        let wrote = self.write(output, &colors, processed);
        self.stats.frame(FrameTimes {
            previous_done: self.previous_done.replace(Instant::now()),
            received,
            captured,
            processed,
            written: wrote.as_ref().ok().copied(),
        });
        let written = wrote?;
        self.feeds
            .publish(&colors, preview, self.processor.viewport());
        self.last = Some((colors, written));
        Ok(())
    }

    fn on_quiet(&mut self, refused: bool, output: &mut dyn LedOutput, now: Instant) -> Result<()> {
        self.stats.no_frame(refused);
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

    #[tokio::test]
    async fn the_final_notice_releases_its_channel_before_the_engine_answers() {
        let (notices, mut events) = notifications::unbounded_channel();
        let answer = request_idle(notices, true);
        let Some(Notice::CaptureClosed { decide_idle, .. }) = events.recv().await else {
            panic!("expected the idle request");
        };
        assert!(matches!(
            events.try_recv(),
            Err(notifications::error::TryRecvError::Disconnected)
        ));
        drop(decide_idle);
        assert!(answer.await.is_err());
    }

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
