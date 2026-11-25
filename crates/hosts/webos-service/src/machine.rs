//! The supervisor's decisions as a pure state machine: inputs and the current
//! time in, effects out. No I/O, no clock; every rule is table-tested.
use crate::preferences::Preferences;
use crate::state::{self, FullWorker, Power, Process, SlimWorker, State};
use prysm_webos_link::{
    Backoff, Command, Escalation, Event, Failure, FailureKind, FullArgs, FullStatus, KILL_AFTER,
    LightingTarget, SILENCE, SlimArgs, SlimLink, SlimStatus, Video, WorkerFailure, WorkerStatus,
    exit,
};
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

mod recovery;
use recovery::{After, Lost, Problem, after, interim_failure};

pub const LOG_LINES: usize = 20;

/// What the machine reacts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Power(Power),
    Video(Video),
    VideoContextChanged,
    /// Already validated and persisted.
    Preferences(Preferences),
    /// Full only: the lighting target the app chose. Slim ignores it (the
    /// service refuses it first).
    Target(LightingTarget),
    /// Slim only: frames should flow. Full ignores it (the service refuses
    /// it first).
    Enabled(bool),
    Worker(Event),
    /// A line for the state's tail: the supervisor's or the worker's log.
    Log(String),
    /// The worker's event stream broke (bad line, oversized, truncated).
    WorkerBroken(String),
    SpawnFailed(String),
    /// Reaped: the process no longer exists.
    WorkerExited(Exit),
    Tick,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Code(i32),
    Signal(i32),
}

/// What the driver must do, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Start the worker with these arguments. Only ever emitted from Idle.
    Spawn(Vec<String>),
    Send(Command),
    Terminate,
    Kill,
    /// Let go of a worker still not reaped [`KILL_AFTER`] after its kill
    /// (`waitpid` keeps failing), so the machine never waits for ever. It
    /// may still hold the capture: the person is asked to restart the TV.
    Abandon,
    /// The published state changed.
    Publish,
}

/// How the worker is started, per package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spec {
    Full {
        settings: PathBuf,
        listen: SocketAddr,
    },
    Slim,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    Idle,
    Running(Run),
    Stopping(Stopping),
    Retrying {
        at: Instant,
    },
    /// No worker: the person is asked. Left only by their or the TV's
    /// action ([`Machine::dismiss`]).
    Asking(Problem),
}

/// A running worker.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Run {
    since: Instant,
    heard: Instant,
    report: Report,
    /// Why it says it fails ([`Event::Failed`]), until it exits.
    failed: Option<String>,
}

/// A worker being stopped: it was sent `sent`; at `at` the next step is due
/// (a kill, then letting go).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stopping {
    at: Instant,
    sent: Sent,
    then: AfterExit,
    /// What it reported until it was stopped and since: its late lines
    /// still explain how it ends.
    report: Report,
    /// Why it says it fails ([`Event::Failed`]; also a fault), until it exits.
    failed: Option<String>,
}

/// What the current worker last reported, in its package's terms.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Report {
    Full {
        /// The `sequence` of the last [`Command::Target`] sent to this
        /// worker; 0 before any (it started with the package's target). A
        /// report that applied an older one does not change the target
        /// back.
        sent: u64,
        status: Option<FullStatus>,
    },
    Slim(Option<SlimStatus>),
}

impl Report {
    /// Nothing reported yet by a worker of `package`.
    fn new(package: &Package) -> Self {
        match package {
            Package::Full { .. } => Self::Full {
                sent: 0,
                status: None,
            },
            Package::Slim { .. } => Self::Slim(None),
        }
    }

    /// Keep `status`; `false` (and nothing kept) when it is the other
    /// package's.
    fn keep(&mut self, status: WorkerStatus) -> bool {
        match (self, status) {
            (Self::Full { status: kept, .. }, WorkerStatus::Full(status)) => *kept = Some(status),
            (Self::Slim(kept), WorkerStatus::Slim(status)) => *kept = Some(status),
            _ => return false,
        }
        true
    }

    /// The reported failure, as the TV app is shown one: a full worker's by
    /// its message alone (kind `other`).
    fn failure(&self) -> Option<WorkerFailure> {
        match self {
            Self::Full { status, .. } => status
                .as_ref()?
                .failure
                .as_ref()
                .map(|failure| WorkerFailure::other(failure.message.clone())),
            Self::Slim(status) => status.as_ref()?.link.failure().cloned(),
        }
    }

    fn full(&self) -> Option<&FullStatus> {
        match self {
            Self::Full { status, .. } => status.as_ref(),
            Self::Slim(_) => None,
        }
    }

    fn slim(&self) -> Option<&SlimStatus> {
        match self {
            Self::Full { .. } => None,
            Self::Slim(status) => status.as_ref(),
        }
    }
}

/// The last signal a stopping worker was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sent {
    Terminate,
    Kill,
}

/// What follows a stopping worker's exit.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AfterExit {
    /// The user or the TV ended it: nothing, or at once the next worker if
    /// one is wanted (new worker settings).
    Idle,
    /// It failed: the next worker after the backoff.
    Retry,
    /// It failed: the person is asked.
    Ask(Problem),
}

/// Why a worker is stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StopCause {
    /// The user or the TV ended it: no longer wanted, or new worker settings.
    Requested,
    /// The supervisor ended it: what follows.
    Failed(After),
}

/// The signal a stop sends.
const SIGTERM: i32 = 15;

/// The package's worker and what the user asked of it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Package {
    Full {
        settings: PathBuf,
        listen: SocketAddr,
        /// The engine target, reported by the worker and used at spawn.
        target: LightingTarget,
    },
    Slim {
        /// Whether frames should flow.
        enabled: bool,
    },
}

impl Package {
    /// What the TV app sees of the package while the worker is at
    /// `process`, with the current worker's `report`.
    fn published(&self, process: Process, report: Option<&Report>) -> state::Package {
        match self {
            Self::Full { target, .. } => state::Package::Full {
                target: *target,
                worker: FullWorker::new(process, report.and_then(Report::full)),
            },
            Self::Slim { enabled } => state::Package::Slim {
                enabled: *enabled,
                worker: SlimWorker::new(process, report.and_then(Report::slim)),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct Machine {
    package: Package,
    preferences: Preferences,
    power: Power,
    video: Video,
    phase: Phase,
    /// Restart delays: reset by a worker that ran [`Backoff::HEALTHY`] and by
    /// a dismissal ([`Machine::dismiss`]).
    backoff: Backoff,
    /// The supervisor's own failure: why the last worker ended or could not
    /// start, or why the current one is being stopped. Set by a failed stop
    /// and by a bad exit ([`Machine::exited`]); dismissed ([`Machine::dismiss`]),
    /// and once the next worker is up again or ran [`Backoff::HEALTHY`].
    failure: Option<WorkerFailure>,
    /// Why the saved preferences could not be read (kind
    /// [`FailureKind::Preferences`]), until the app saves new ones. Shown
    /// only while no other failure is current.
    unreadable: Option<WorkerFailure>,
    log: VecDeque<String>,
    shutdown: bool,
}

impl Machine {
    /// Intent starts from `autostart`: enabled (full: screen sync) or not.
    #[must_use]
    pub fn new(spec: Spec, preferences: Preferences) -> Self {
        let intent = if preferences.autostart {
            LightingTarget::ScreenSync
        } else {
            LightingTarget::Off
        };
        Self::resumed(spec, preferences, intent)
    }

    /// Intent starts from what was asked for before a restart (see
    /// [`Machine::intent`]), whatever `autostart` says.
    #[must_use]
    pub fn resumed(spec: Spec, preferences: Preferences, intent: LightingTarget) -> Self {
        Self {
            package: match spec {
                Spec::Full { settings, listen } => Package::Full {
                    settings,
                    listen,
                    target: intent,
                },
                Spec::Slim => Package::Slim {
                    enabled: intent != LightingTarget::Off,
                },
            },
            preferences,
            power: Power::Unknown,
            video: Video::Unknown,
            phase: Phase::Idle,
            backoff: Backoff::default(),
            failure: None,
            unreadable: None,
            log: VecDeque::new(),
            shutdown: false,
        }
    }

    /// The saved preferences could not be read (`why`): the machine runs
    /// with the ones it was given and reports this until the app saves
    /// preferences ([`Input::Preferences`]).
    #[must_use]
    pub fn with_unreadable_preferences(mut self, why: impl Into<String>) -> Self {
        self.unreadable = Some(WorkerFailure::new(FailureKind::Preferences, why));
        self
    }

    #[must_use]
    pub fn preferences(&self) -> &Preferences {
        &self.preferences
    }

    /// What the user asked for: full, the lighting target (also when changed
    /// in the portal); slim, `ScreenSync` while enabled, else `Off`.
    #[must_use]
    pub fn intent(&self) -> LightingTarget {
        match self.package {
            Package::Full { target, .. } => target,
            Package::Slim { enabled: true, .. } => LightingTarget::ScreenSync,
            Package::Slim { enabled: false, .. } => LightingTarget::Off,
        }
    }

    /// Shut down and no worker left.
    #[must_use]
    pub fn finished(&self) -> bool {
        self.shutdown && matches!(self.phase, Phase::Idle | Phase::Asking(_))
    }

    pub fn handle(&mut self, input: Input, now: Instant) -> Vec<Effect> {
        let before = self.state(now);
        let mut effects = Vec::new();
        let video_before = self.effective_video();
        match input {
            Input::Power(power) => {
                if self.power.awake() && !power.awake() {
                    self.dismiss();
                }
                self.power = power;
            }
            Input::Video(video) => self.video = video,
            Input::VideoContextChanged => {
                if self.running() {
                    effects.push(Effect::Send(Command::ResetVideo {}));
                }
            }
            Input::Preferences(preferences) => self.set_preferences(preferences, now, &mut effects),
            Input::Target(target) => self.set_target(target, &mut effects),
            Input::Enabled(enabled) => self.set_enabled(enabled),
            Input::Worker(event) => self.worker_event(event, now, &mut effects),
            Input::Log(line) => {
                if self.log.len() == LOG_LINES {
                    self.log.pop_front();
                }
                self.log.push_back(line);
            }
            Input::WorkerBroken(why) => self.lose(&Lost::Broken(why), now, &mut effects),
            Input::SpawnFailed(why) => {
                if self.running() {
                    self.gone(after(Lost::Spawn(why).failure()), now);
                }
            }
            Input::WorkerExited(exit) => self.exited(exit, now),
            Input::Tick => self.tick(now, &mut effects),
            Input::Shutdown => {
                self.dismiss();
                self.shutdown = true;
            }
        }
        let video_after = self.effective_video();
        if self.running() && video_after != video_before {
            effects.push(Effect::Send(Command::Video { video: video_after }));
        }
        self.reconcile(now, &mut effects);
        if self.state(now) != before {
            effects.push(Effect::Publish);
        }
        effects
    }

    /// The state the TV app sees at `now`.
    #[must_use]
    pub fn state(&self, now: Instant) -> State {
        State {
            package: self.package.published(self.process(now), self.reported()),
            preferences: self.preferences.clone(),
            power: self.power,
            video: self.video,
            failure: self.current_failure(),
            log: self.log.iter().cloned().collect(),
        }
    }

    fn process(&self, now: Instant) -> Process {
        match self.phase {
            Phase::Idle | Phase::Asking(_) => Process::Stopped,
            Phase::Running(_) => Process::Running,
            Phase::Stopping(_) => Process::Stopping,
            Phase::Retrying { at } => Process::Retrying {
                seconds: whole_seconds(at.saturating_duration_since(now)),
            },
        }
    }

    /// The current worker's report; none without a worker.
    fn reported(&self) -> Option<&Report> {
        match &self.phase {
            Phase::Running(run) => Some(&run.report),
            Phase::Stopping(stopping) => Some(&stopping.report),
            Phase::Idle | Phase::Retrying { .. } | Phase::Asking(_) => None,
        }
    }

    fn failed(&self) -> Option<&String> {
        match &self.phase {
            Phase::Running(run) => run.failed.as_ref(),
            Phase::Stopping(stopping) => stopping.failed.as_ref(),
            Phase::Idle | Phase::Retrying { .. } | Phase::Asking(_) => None,
        }
    }

    /// What the person is asked comes first, also while the worker that
    /// failed stops. Then the worker's own word (first why it is failing
    /// now, then its report) wins over why the last one ended; while the
    /// supervisor stops it as failed, that reason wins. Unreadable
    /// preferences come last: the others name the fix for what is failing
    /// now.
    fn current_failure(&self) -> Option<WorkerFailure> {
        let worker = || {
            self.failed()
                .cloned()
                .map(WorkerFailure::other)
                .or_else(|| self.reported().and_then(Report::failure))
        };
        match &self.phase {
            Phase::Asking(problem)
            | Phase::Stopping(Stopping {
                then: AfterExit::Ask(problem),
                ..
            }) => Some(interim_failure(problem)),
            Phase::Stopping(Stopping {
                then: AfterExit::Retry,
                ..
            }) => self.failure.clone().or_else(worker),
            Phase::Idle
            | Phase::Running(_)
            | Phase::Stopping(Stopping {
                then: AfterExit::Idle,
                ..
            })
            | Phase::Retrying { .. } => worker().or_else(|| self.failure.clone()),
        }
        .or_else(|| self.unreadable.clone())
    }

    fn running(&self) -> bool {
        matches!(self.phase, Phase::Running(_))
    }

    fn effective_video(&self) -> Video {
        if self.power == Power::Active {
            self.video
        } else {
            Video::Unavailable
        }
    }

    fn wanted(&self) -> bool {
        self.spawn_arguments().is_some()
    }

    /// The worker's arguments while one is wanted: awake and not shut down,
    /// and for slim enabled with a receiver.
    fn spawn_arguments(&self) -> Option<Vec<String>> {
        if self.shutdown || !self.power.awake() {
            return None;
        }
        match &self.package {
            // The portal stays up while awake, whatever the lighting.
            Package::Full {
                settings,
                listen,
                target,
                ..
            } => Some(
                FullArgs {
                    settings: settings.clone(),
                    listen: *listen,
                    target: *target,
                }
                .to_args(),
            ),
            Package::Slim { enabled, .. } => {
                let prysm = self
                    .preferences
                    .prysm_address
                    .clone()
                    .filter(|_| *enabled)?;
                Some(SlimArgs { prysm }.to_args())
            }
        }
    }

    fn reconcile(&mut self, now: Instant, effects: &mut Vec<Effect>) {
        match self.phase {
            Phase::Idle => {
                if let Some(arguments) = self.spawn_arguments() {
                    self.spawn(arguments, now, effects);
                }
            }
            Phase::Running(_)
            | Phase::Stopping(Stopping {
                then: AfterExit::Retry | AfterExit::Ask(_),
                ..
            })
            | Phase::Retrying { .. }
                if !self.wanted() =>
            {
                self.stop(StopCause::Requested, now, effects);
            }
            Phase::Retrying { at } if now >= at => {
                self.phase = Phase::Idle;
                self.reconcile(now, effects);
            }
            // Asking ends only by the person's or the TV's action.
            Phase::Running(_) | Phase::Stopping(_) | Phase::Retrying { .. } | Phase::Asking(_) => {}
        }
    }

    fn spawn(&mut self, arguments: Vec<String>, now: Instant, effects: &mut Vec<Effect>) {
        effects.push(Effect::Spawn(arguments));
        effects.push(Effect::Send(Command::Video {
            video: self.effective_video(),
        }));
        self.phase = Phase::Running(Run {
            since: now,
            heard: now,
            report: Report::new(&self.package),
            failed: None,
        });
    }

    /// End the worker for `cause`. A failed stop ends only a running worker
    /// and sets why. A requested stop works in any phase (it also turns a
    /// failed stop into a requested one and ends a backoff) and dismisses
    /// the failure.
    fn stop(&mut self, cause: StopCause, now: Instant, effects: &mut Vec<Effect>) {
        let then = match cause {
            StopCause::Requested => {
                self.dismiss();
                AfterExit::Idle
            }
            StopCause::Failed(after) => {
                if !self.running() {
                    return;
                }
                match after {
                    After::Restart(why) => {
                        self.failure = Some(WorkerFailure::other(why));
                        AfterExit::Retry
                    }
                    After::Ask(problem) => AfterExit::Ask(problem),
                }
            }
        };
        self.phase = match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Running(run) => {
                effects.push(Effect::Terminate);
                Phase::Stopping(Stopping {
                    at: now + KILL_AFTER,
                    sent: Sent::Terminate,
                    then,
                    report: run.report,
                    failed: run.failed,
                })
            }
            Phase::Stopping(stopping) => Phase::Stopping(Stopping { then, ..stopping }),
            Phase::Idle | Phase::Retrying { .. } | Phase::Asking(_) => Phase::Idle,
        };
    }

    /// The user or the TV ended the worker (turned off, standby, shutdown,
    /// new worker settings) or answered what it was asked: the failure is
    /// no longer current, and the next start gets a fresh backoff. The one
    /// place that clears the failure besides a worker that is up again or
    /// ran [`Backoff::HEALTHY`], and the one way out of asking.
    fn dismiss(&mut self) {
        self.failure = None;
        self.backoff.reset();
        if let Phase::Asking(_) = self.phase {
            self.phase = Phase::Idle;
        }
    }

    /// Stop a running worker as failed, for `why`; the next one after the
    /// backoff.
    fn fail(&mut self, why: String, now: Instant, effects: &mut Vec<Effect>) {
        self.stop(StopCause::Failed(After::Restart(why)), now, effects);
    }

    /// Stop a running worker the supervisor lost for `lost`.
    fn lose(&mut self, lost: &Lost, now: Instant, effects: &mut Vec<Effect>) {
        self.stop(StopCause::Failed(after(lost.failure())), now, effects);
    }

    /// What follows a worker that is gone (exited, never started or let
    /// go): the next one after the backoff, or the person is asked.
    fn gone(&mut self, after: After, now: Instant) {
        match after {
            After::Restart(why) => {
                self.failure = Some(WorkerFailure::other(why));
                self.retry(now);
            }
            After::Ask(problem) => self.phase = Phase::Asking(problem),
        }
    }

    fn retry(&mut self, now: Instant) {
        self.phase = Phase::Retrying {
            at: now + self.backoff.delay(),
        };
    }

    fn set_preferences(
        &mut self,
        preferences: Preferences,
        now: Instant,
        effects: &mut Vec<Effect>,
    ) {
        let relaunch = matches!(self.package, Package::Slim { .. })
            && self.preferences.prysm_address != preferences.prysm_address;
        self.preferences = preferences;
        // Saved: the file is readable again.
        self.unreadable = None;
        if relaunch {
            // New worker settings: the old worker ends, and `reconcile`
            // starts the new one as soon as it is gone, without backoff.
            self.stop(StopCause::Requested, now, effects);
        }
    }

    /// Full: the worker stays wanted whatever the target, so its failure
    /// and backoff stay current; a running one is sent the target, with the
    /// next sequence number.
    fn set_target(&mut self, target: LightingTarget, effects: &mut Vec<Effect>) {
        let Package::Full {
            target: current, ..
        } = &mut self.package
        else {
            return;
        };
        *current = target;
        // Asked: the person answers by choosing a target.
        if let Phase::Asking(_) = self.phase {
            self.dismiss();
        }
        if let Phase::Running(Run {
            report: Report::Full { sent, .. },
            ..
        }) = &mut self.phase
        {
            *sent += 1;
            effects.push(Effect::Send(Command::Target {
                target,
                sequence: *sent,
            }));
        }
    }

    /// Slim: off ends the worker (`reconcile` stops one no longer wanted).
    fn set_enabled(&mut self, enabled: bool) {
        let Package::Slim { enabled: current } = &mut self.package else {
            return;
        };
        *current = enabled;
        // Off ends the worker; on answers what the person was asked.
        if !enabled || matches!(self.phase, Phase::Asking(_)) {
            self.dismiss();
        }
    }

    fn worker_event(&mut self, event: Event, now: Instant, effects: &mut Vec<Effect>) {
        let run = match &mut self.phase {
            Phase::Running(run) => run,
            // A late line from a worker already being stopped: its report
            // and why it fails still explain how it ends.
            Phase::Stopping(stopping) => {
                match event {
                    Event::Status(status) => {
                        stopping.report.keep(status);
                    }
                    Event::Failed(failure) => stopping.failed = Some(failure.message),
                }
                return;
            }
            Phase::Idle | Phase::Retrying { .. } | Phase::Asking(_) => return,
        };
        run.heard = now;
        match event {
            Event::Status(status) => self.report(status, now, effects),
            Event::Failed(failure) => match interim_fault(&failure) {
                Some(why) => self.fail(why, now, effects),
                None => run.failed = Some(failure.message),
            },
        }
    }

    /// A running worker's report. A full worker's reported fault stops it
    /// as failed ([`interim_fault`]).
    fn report(&mut self, status: WorkerStatus, now: Instant, effects: &mut Vec<Effect>) {
        // Up again (full: reporting without a failure; slim: it reached the
        // receiver, which takes its frames, waits for video the TV refuses to
        // share or paused the stream): the last
        // worker's failure is no longer current.
        let (up, target, fault) = match &status {
            WorkerStatus::Full(full) => (
                full.failure.is_none(),
                Some((full.target, full.applied)),
                full.failure.as_ref().and_then(interim_fault),
            ),
            WorkerStatus::Slim(slim) => (
                matches!(
                    slim.link,
                    SlimLink::Sending { .. } | SlimLink::Refused {} | SlimLink::Paused {}
                ),
                None,
                None,
            ),
        };
        let Phase::Running(run) = &mut self.phase else {
            return;
        };
        if !run.report.keep(status) {
            let why = "the worker reported the other package's status";
            self.lose(&Lost::Broken(why.into()), now, effects);
            return;
        }
        if up {
            self.failure = None;
        }
        if let Some((target, applied)) = target {
            self.follow(target, applied);
        }
        if let Some(why) = fault {
            self.fail(why, now, effects);
        }
    }

    /// Full: take the worker's target (also changed in the portal) from a
    /// report that `applied` the last target sent; an older report was sent
    /// before the worker read it.
    fn follow(&mut self, target: LightingTarget, applied: u64) {
        if let Package::Full {
            target: current, ..
        } = &mut self.package
            && let Phase::Running(Run {
                report: Report::Full { sent, .. },
                ..
            }) = &self.phase
            && applied >= *sent
        {
            *current = target;
        }
    }

    /// A worker on its own stops only when something failed; a stopping one
    /// ends as asked, unless it faulted, died of a signal other than SIGTERM
    /// or had to be killed: then it could not release what it held, and that is a
    /// failure even after a requested stop.
    fn exited(&mut self, exit: Exit, now: Instant) {
        let last = self
            .failed()
            .cloned()
            .map(WorkerFailure::other)
            .or_else(|| self.reported().and_then(Report::failure));
        let described = match exit {
            Exit::Code(code) => exit::describe(code),
            Exit::Signal(signal) => format!("worker killed by signal {signal}"),
        };
        // A SIGTERM that came before the worker's handlers were in is the
        // stop that was asked for.
        let faulted = matches!(exit, Exit::Signal(signal) if signal != SIGTERM)
            || exit == Exit::Code(exit::FAULTED.into());
        match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Running(_) => {
                self.failure = Some(exit_failure(described, last));
                self.retry(now);
            }
            // The failure that stopped it is already set.
            Phase::Stopping(Stopping {
                then: AfterExit::Retry,
                ..
            }) => self.retry(now),
            Phase::Stopping(Stopping {
                then: AfterExit::Ask(problem),
                ..
            }) => self.phase = Phase::Asking(problem),
            Phase::Stopping(Stopping {
                sent: Sent::Kill,
                then: AfterExit::Idle,
                ..
            }) => {
                let described = format!("worker did not stop within {} s", KILL_AFTER.as_secs());
                self.failure = Some(exit_failure(described, last));
                self.phase = Phase::Idle;
            }
            Phase::Stopping(Stopping {
                sent: Sent::Terminate,
                then: AfterExit::Idle,
                ..
            }) => {
                if faulted {
                    self.failure = Some(exit_failure(described, last));
                }
                self.phase = Phase::Idle;
            }
            phase @ (Phase::Idle | Phase::Retrying { .. } | Phase::Asking(_)) => self.phase = phase,
        }
    }

    /// When [`Input::Tick`] next has something to do, after `now`: a
    /// running worker's [`Backoff::HEALTHY`] and [`SILENCE`], a stopping
    /// one's kill or letting go, the end of a backoff. `None`: only an input
    /// changes anything.
    #[must_use]
    pub fn next_due(&self, now: Instant) -> Option<Instant> {
        match self.phase {
            Phase::Idle | Phase::Asking(_) => None,
            Phase::Running(Run { since, heard, .. }) => {
                let healthy = Some(since + Backoff::HEALTHY).filter(|&at| at > now);
                healthy.into_iter().chain([heard + SILENCE]).min()
            }
            Phase::Stopping(Stopping { at, .. }) | Phase::Retrying { at } => Some(at),
        }
    }

    fn tick(&mut self, now: Instant, effects: &mut Vec<Effect>) {
        match &mut self.phase {
            Phase::Running(Run { since, heard, .. }) => {
                let (since, heard) = (*since, *heard);
                if now.duration_since(since) >= Backoff::HEALTHY {
                    self.backoff.reset();
                    self.failure = None;
                }
                if now.duration_since(heard) >= SILENCE {
                    self.lose(&Lost::Silent, now, effects);
                }
            }
            Phase::Stopping(stopping) if now >= stopping.at => match stopping.sent {
                Sent::Terminate => {
                    effects.push(Effect::Kill);
                    stopping.at = now + KILL_AFTER;
                    stopping.sent = Sent::Kill;
                }
                // It may still hold the capture, however it was stopped.
                Sent::Kill => {
                    effects.push(Effect::Abandon);
                    self.gone(after(Lost::Abandoned.failure()), now);
                }
            },
            Phase::Idle | Phase::Stopping(_) | Phase::Retrying { .. } | Phase::Asking(_) => {}
        }
    }
}

/// Interim, deleted by the supervisor's recovery, which reads every
/// escalation in one exhaustive match: what a running worker's
/// [`Event::Failed`], or a running full worker's reported failure, meant
/// before failures carried their escalation. A
/// `Restart` was the worker's fault: why it is stopped as failed at once,
/// to be restarted after the backoff. Any other escalation (`None`) was a
/// failure kept until the worker exits.
fn interim_fault(failure: &Failure) -> Option<String> {
    match failure.escalation {
        Escalation::Restart => Some(format!("worker faulted: {}", failure.message)),
        Escalation::Unanswered | Escalation::Ask(_) => None,
    }
}

/// The failure a worker's end is: `described`, with the worker's last
/// failure. An encoder, version or replaced report keeps its kind (the TV
/// app offers its fix); a receiver wait or a failed receiver is no reason
/// for this worker to exit, so `other`.
fn exit_failure(described: String, last: Option<WorkerFailure>) -> WorkerFailure {
    match last {
        Some(last) => WorkerFailure {
            kind: match last.kind {
                FailureKind::Receiver | FailureKind::ReceiverFailed => FailureKind::Other,
                kind => kind,
            },
            message: format!("{described}: {}", last.message),
        },
        None => WorkerFailure::other(described),
    }
}

/// `duration` in whole seconds, rounded up.
fn whole_seconds(duration: Duration) -> u32 {
    u32::try_from(duration.as_millis().div_ceil(1000)).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests;
