//! The supervisor's decisions as a pure state machine: inputs and the current
//! time in, effects out. No I/O, no clock; every rule is table-tested.
use crate::preferences::Preferences;
use crate::state::{self, FullWorker, Power, Process, SlimWorker, State};
use prysm_webos_link::{
    Backoff, Command, Event, FailureKind, FullArgs, FullStatus, KILL_AFTER, LightingTarget,
    SILENCE, SlimArgs, SlimLink, SlimStatus, Video, WorkerFailure, WorkerStatus, exit,
};
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How long a sent `Lighting` target wins over older worker reports.
pub const ECHO: Duration = Duration::from_secs(2);
pub const LOG_LINES: usize = 20;

/// What the machine reacts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Power(Power),
    Video(Video),
    VideoContextChanged,
    /// Already validated and persisted.
    Preferences(Preferences),
    Enabled(bool),
    Worker(Event),
    WorkerLog(String),
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
    /// (`waitpid` keeps failing): the machine takes it as killed, so it never
    /// waits for ever.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Running {
        since: Instant,
        heard: Instant,
    },
    /// The worker was sent `sent`; at `at` the next step is due (a kill,
    /// then letting go).
    Stopping {
        at: Instant,
        sent: Sent,
        then: AfterExit,
    },
    Retrying {
        at: Instant,
    },
}

/// The last signal a stopping worker was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sent {
    Terminate,
    Kill,
}

/// What follows a stopping worker's exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AfterExit {
    /// The user or the TV ended it: nothing, or at once the next worker if
    /// one is wanted (new worker settings).
    Idle,
    /// It failed: the next worker after the backoff.
    Retry,
}

/// Why a worker is stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StopCause {
    /// The user or the TV ended it: no longer wanted, or new worker settings.
    Requested,
    /// The supervisor ended it, and why.
    Failed(WorkerFailure),
}

/// The signal a kill sends; also what a worker let go is taken to have died of.
const SIGKILL: i32 = 9;
/// The signal a stop sends.
const SIGTERM: i32 = 15;

/// The package's worker, what the user asked of it and what the current
/// worker last reported, in its terms.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Package {
    Full {
        settings: PathBuf,
        listen: SocketAddr,
        /// The engine target, reported by the worker and used at spawn.
        lighting: LightingTarget,
        report: Option<FullStatus>,
    },
    Slim {
        /// Whether frames should flow.
        enabled: bool,
        report: Option<SlimStatus>,
    },
}

impl Package {
    /// Keep `status` as the current worker's report; `false` (and nothing
    /// kept) when it is the other package's.
    fn keep(&mut self, status: WorkerStatus) -> bool {
        match (self, status) {
            (Self::Full { report, .. }, WorkerStatus::Full(status)) => *report = Some(status),
            (Self::Slim { report, .. }, WorkerStatus::Slim(status)) => *report = Some(status),
            _ => return false,
        }
        true
    }

    /// Forget the report of a worker that is gone; its failure, if any.
    fn forget(&mut self) -> Option<WorkerFailure> {
        match self {
            Self::Full { report, .. } => report.take().and_then(|report| report.failure),
            Self::Slim { report, .. } => report
                .take()
                .and_then(|report| report.link.failure().cloned()),
        }
    }

    fn reported_failure(&self) -> Option<&WorkerFailure> {
        match self {
            Self::Full { report, .. } => report.as_ref()?.failure.as_ref(),
            Self::Slim { report, .. } => report.as_ref()?.link.failure(),
        }
    }

    /// What the TV app sees of the package while the worker is at `process`.
    fn published(&self, process: Process) -> state::Package {
        match self {
            Self::Full {
                lighting, report, ..
            } => state::Package::Full {
                lighting: *lighting,
                worker: FullWorker::new(process, report.as_ref()),
            },
            Self::Slim { enabled, report } => state::Package::Slim {
                enabled: *enabled,
                worker: SlimWorker::new(process, report.as_ref()),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct Machine {
    package: Package,
    preferences: Preferences,
    echo_until: Option<Instant>,
    power: Power,
    video: Video,
    phase: Phase,
    /// Restart delays: reset by a worker that ran [`Backoff::HEALTHY`] and by
    /// a dismissal ([`Machine::dismiss`]).
    backoff: Backoff,
    /// Why the current worker says it fails or faults ([`Event::Failed`],
    /// [`Event::Faulted`] while it stops), until it exits.
    failed: Option<String>,
    /// The supervisor's own failure: why the last worker ended or could not
    /// start, or why the current one is being stopped. Set by a failed stop
    /// and by a bad exit ([`Machine::exited`]); dismissed ([`Machine::dismiss`]),
    /// and once the next worker is up again or ran [`Backoff::HEALTHY`].
    failure: Option<WorkerFailure>,
    log: VecDeque<String>,
    shutdown: bool,
}

impl Machine {
    /// Intent starts from `autostart`: enabled (full: processed lighting) or not.
    #[must_use]
    pub fn new(spec: Spec, preferences: Preferences) -> Self {
        let intent = if preferences.autostart {
            LightingTarget::Processed
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
                    lighting: intent,
                    report: None,
                },
                Spec::Slim => Package::Slim {
                    enabled: intent != LightingTarget::Off,
                    report: None,
                },
            },
            preferences,
            echo_until: None,
            power: Power::Unknown,
            video: Video::Unknown,
            phase: Phase::Idle,
            backoff: Backoff::default(),
            failed: None,
            failure: None,
            log: VecDeque::new(),
            shutdown: false,
        }
    }

    #[must_use]
    pub fn preferences(&self) -> &Preferences {
        &self.preferences
    }

    /// What the user asked for: full, the lighting target (also when changed
    /// in the portal); slim, `Processed` while enabled, else `Off`.
    #[must_use]
    pub fn intent(&self) -> LightingTarget {
        match self.package {
            Package::Full { lighting, .. } => lighting,
            Package::Slim { enabled: true, .. } => LightingTarget::Processed,
            Package::Slim { enabled: false, .. } => LightingTarget::Off,
        }
    }

    /// Shut down and no worker left.
    #[must_use]
    pub fn finished(&self) -> bool {
        self.shutdown && self.phase == Phase::Idle
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
            Input::Enabled(enabled) => self.set_enabled(enabled, now, &mut effects),
            Input::Worker(event) => self.worker_event(event, now, &mut effects),
            Input::WorkerLog(line) => {
                if self.log.len() == LOG_LINES {
                    self.log.pop_front();
                }
                self.log.push_back(line);
            }
            Input::WorkerBroken(why) => {
                self.fail(&format!("worker link broke: {why}"), now, &mut effects);
            }
            Input::SpawnFailed(why) => {
                if self.running() {
                    self.failure = Some(WorkerFailure::other(format!(
                        "cannot start the worker: {why}"
                    )));
                    self.retry(now);
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
            package: self.package.published(self.process(now)),
            preferences: self.preferences.clone(),
            power: self.power,
            video: self.video,
            failure: self.current_failure(),
            log: self.log.iter().cloned().collect(),
        }
    }

    fn process(&self, now: Instant) -> Process {
        match self.phase {
            Phase::Idle => Process::Stopped,
            Phase::Running { .. } => Process::Running,
            Phase::Stopping { .. } => Process::Stopping,
            Phase::Retrying { at } => Process::Retrying {
                seconds: whole_seconds(at.saturating_duration_since(now)),
            },
        }
    }

    /// The worker's own word (first why it is failing now, then its report)
    /// wins over why the last one ended; while the supervisor stops it as
    /// failed, that reason wins.
    fn current_failure(&self) -> Option<WorkerFailure> {
        let worker = || {
            self.failed
                .clone()
                .map(WorkerFailure::other)
                .or_else(|| self.package.reported_failure().cloned())
        };
        match self.phase {
            Phase::Stopping {
                then: AfterExit::Retry,
                ..
            } => self.failure.clone().or_else(worker),
            _ => worker().or_else(|| self.failure.clone()),
        }
    }

    fn running(&self) -> bool {
        matches!(self.phase, Phase::Running { .. })
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
                lighting,
                ..
            } => Some(
                FullArgs {
                    settings: settings.clone(),
                    listen: *listen,
                    lighting: *lighting,
                }
                .to_args(),
            ),
            Package::Slim { enabled, .. } => {
                let prysm = self.preferences.instance.clone().filter(|_| *enabled)?;
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
            Phase::Running { .. }
            | Phase::Stopping {
                then: AfterExit::Retry,
                ..
            }
            | Phase::Retrying { .. }
                if !self.wanted() =>
            {
                self.stop(StopCause::Requested, now, effects);
            }
            Phase::Retrying { at } if now >= at => {
                self.phase = Phase::Idle;
                self.reconcile(now, effects);
            }
            Phase::Running { .. } | Phase::Stopping { .. } | Phase::Retrying { .. } => {}
        }
    }

    fn spawn(&mut self, arguments: Vec<String>, now: Instant, effects: &mut Vec<Effect>) {
        effects.push(Effect::Spawn(arguments));
        effects.push(Effect::Send(Command::Video {
            video: self.effective_video(),
        }));
        self.phase = Phase::Running {
            since: now,
            heard: now,
        };
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
            StopCause::Failed(failure) => {
                if !self.running() {
                    return;
                }
                self.failure = Some(failure);
                AfterExit::Retry
            }
        };
        self.phase = match self.phase {
            Phase::Running { .. } => {
                effects.push(Effect::Terminate);
                Phase::Stopping {
                    at: now + KILL_AFTER,
                    sent: Sent::Terminate,
                    then,
                }
            }
            Phase::Stopping { at, sent, .. } => Phase::Stopping { at, sent, then },
            Phase::Idle | Phase::Retrying { .. } => Phase::Idle,
        };
    }

    /// The user or the TV ended the worker (turned off, standby, shutdown,
    /// new worker settings): the failure is no longer current, and the next
    /// start gets a fresh backoff. The one place that clears the failure
    /// besides a worker that is up again or ran [`Backoff::HEALTHY`].
    fn dismiss(&mut self) {
        self.failure = None;
        self.backoff.reset();
    }

    /// Stop a running worker as failed, for `why`.
    fn fail(&mut self, why: &str, now: Instant, effects: &mut Vec<Effect>) {
        self.stop(StopCause::Failed(WorkerFailure::other(why)), now, effects);
    }

    fn retry(&mut self, now: Instant) {
        self.package.forget();
        self.failed = None;
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
            && self.preferences.instance != preferences.instance;
        self.preferences = preferences;
        if relaunch {
            // New worker settings: the old worker ends, and `reconcile`
            // starts the new one as soon as it is gone, without backoff.
            self.stop(StopCause::Requested, now, effects);
        }
    }

    fn set_enabled(&mut self, enabled: bool, now: Instant, effects: &mut Vec<Effect>) {
        // Off ends the slim worker; the full worker stays wanted, so its
        // failure and backoff stay current.
        if !enabled && matches!(self.package, Package::Slim { .. }) {
            self.dismiss();
        }
        let running = self.running();
        match &mut self.package {
            Package::Slim {
                enabled: current, ..
            } => *current = enabled,
            Package::Full { lighting, .. } => {
                *lighting = match (enabled, *lighting) {
                    (false, _) => LightingTarget::Off,
                    (true, LightingTarget::Off) => LightingTarget::Processed,
                    (true, current) => current,
                };
                if running {
                    effects.push(Effect::Send(Command::Lighting { target: *lighting }));
                    self.echo_until = Some(now + ECHO);
                }
            }
        }
    }

    fn worker_event(&mut self, event: Event, now: Instant, effects: &mut Vec<Effect>) {
        let Phase::Running { since, .. } = self.phase else {
            // A late line from a worker already being stopped: its report
            // and why it fails still explain how it ends.
            if let Phase::Stopping { .. } = self.phase {
                match event {
                    Event::Status(status) => {
                        self.package.keep(status);
                    }
                    Event::Failed { cause } | Event::Faulted { cause } => self.failed = Some(cause),
                }
            }
            return;
        };
        self.phase = Phase::Running { since, heard: now };
        match event {
            Event::Status(status) => self.report(status, now, effects),
            Event::Failed { cause } => self.failed = Some(cause),
            Event::Faulted { cause } => {
                self.fail(&format!("worker faulted: {cause}"), now, effects)
            }
        }
    }

    /// A running worker's report.
    fn report(&mut self, status: WorkerStatus, now: Instant, effects: &mut Vec<Effect>) {
        // Up again (full: reporting without a failure; slim: it reached the
        // receiver, which takes its frames or paused the stream): the last
        // worker's failure is no longer current.
        let (up, target) = match &status {
            WorkerStatus::Full(full) => (full.failure.is_none(), Some(full.target)),
            WorkerStatus::Slim(slim) => (
                matches!(slim.link, SlimLink::Sending { .. } | SlimLink::Paused {}),
                None,
            ),
        };
        if !self.package.keep(status) {
            let why = "worker link broke: the worker reported the other package's status";
            self.fail(why, now, effects);
            return;
        }
        if up {
            self.failure = None;
        }
        if let Some(target) = target {
            self.follow(target, now);
        }
    }

    /// Full: take the worker's target (also changed in the portal), unless a
    /// target sent within [`ECHO`] is still on its way to the worker.
    fn follow(&mut self, target: LightingTarget, now: Instant) {
        if let Package::Full { lighting, .. } = &mut self.package {
            let echoed = self.echo_until.is_none_or(|until| now >= until);
            if echoed || target == *lighting {
                *lighting = target;
                self.echo_until = None;
            }
        }
    }

    /// A worker on its own stops only when something failed; a stopping one
    /// ends as asked, unless it faulted, died of a signal other than SIGTERM
    /// or had to be killed: then it could not release what it held, and that is a
    /// failure even after a requested stop.
    fn exited(&mut self, exit: Exit, now: Instant) {
        let reported = self.package.forget();
        let last = self.failed.take().map(WorkerFailure::other).or(reported);
        let described = match exit {
            Exit::Code(code) => exit::describe(code),
            Exit::Signal(signal) => format!("worker killed by signal {signal}"),
        };
        // A SIGTERM that came before the worker's handlers were in is the
        // stop that was asked for.
        let faulted = matches!(exit, Exit::Signal(signal) if signal != SIGTERM)
            || exit == Exit::Code(exit::FAULTED.into());
        match self.phase {
            Phase::Running { .. } => {
                self.failure = Some(exit_failure(described, last));
                self.retry(now);
            }
            // The failure that stopped it is already set.
            Phase::Stopping {
                then: AfterExit::Retry,
                ..
            } => self.retry(now),
            Phase::Stopping {
                sent: Sent::Kill,
                then: AfterExit::Idle,
                ..
            } => {
                let described = format!("worker did not stop within {} s", KILL_AFTER.as_secs());
                self.failure = Some(exit_failure(described, last));
                self.phase = Phase::Idle;
            }
            Phase::Stopping {
                sent: Sent::Terminate,
                then: AfterExit::Idle,
                ..
            } => {
                if faulted {
                    self.failure = Some(exit_failure(described, last));
                }
                self.phase = Phase::Idle;
            }
            Phase::Idle | Phase::Retrying { .. } => {}
        }
    }

    fn tick(&mut self, now: Instant, effects: &mut Vec<Effect>) {
        match self.phase {
            Phase::Running { since, heard } => {
                if now.duration_since(since) >= Backoff::HEALTHY {
                    self.backoff.reset();
                    self.failure = None;
                }
                if now.duration_since(heard) >= SILENCE {
                    let why = format!("worker sent nothing for {} s", SILENCE.as_secs());
                    self.fail(&why, now, effects);
                }
            }
            Phase::Stopping { at, sent, then } if now >= at => match sent {
                Sent::Terminate => {
                    effects.push(Effect::Kill);
                    self.phase = Phase::Stopping {
                        at: now + KILL_AFTER,
                        sent: Sent::Kill,
                        then,
                    };
                }
                Sent::Kill => {
                    effects.push(Effect::Abandon);
                    self.exited(Exit::Signal(SIGKILL), now);
                    // It may still hold the capture: the next one waits.
                    if self.phase == Phase::Idle && self.wanted() {
                        self.retry(now);
                    }
                }
            },
            _ => {}
        }
        if self.echo_until.is_some_and(|until| now >= until) {
            self.echo_until = None;
        }
    }
}

/// The failure a worker's end is: `described`, with the worker's last
/// failure. An encoder, version or replaced report keeps its kind (the TV
/// app offers its fix); a receiver wait is no reason to exit, so `other`.
fn exit_failure(described: String, last: Option<WorkerFailure>) -> WorkerFailure {
    match last {
        Some(last) => WorkerFailure {
            kind: match last.kind {
                FailureKind::Receiver => FailureKind::Other,
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
