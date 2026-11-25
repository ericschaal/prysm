//! The supervisor's decisions as a pure state machine: inputs and the current
//! time in, effects out. No I/O, no clock; every rule is table-tested.
//!
//! - [`phase`]: where the worker is, with what is valid only there.
//! - [`recovery`]: what follows a failed worker, the one reading of an
//!   escalation and its one automatic recovery, a restart.
use crate::preferences::Preferences;
use crate::state::{self, FullWorker, Power, Process, SlimWorker, State};
use prysm_webos_link::{
    Command, Event, FullArgs, KILL_AFTER, LightingTarget, SILENCE, SlimArgs, Video, WorkerStatus,
};
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Instant;

mod phase;
mod recovery;
use phase::{Phase, Report, Run, Sent, Stopping, Then};
use recovery::{After, Lost, Restart, after};

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
    /// Start the worker with these arguments. Only ever emitted without a
    /// worker: before the first, or once the last was reaped.
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
    /// `process`, with the running worker's `report`.
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
    /// Why the saved preferences could not be read, until the app saves
    /// new ones.
    unreadable: Option<String>,
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
        self.unreadable = Some(why.into());
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
            Package::Slim { enabled: true } => LightingTarget::ScreenSync,
            Package::Slim { enabled: false } => LightingTarget::Off,
        }
    }

    /// Shut down and no worker left.
    #[must_use]
    pub fn finished(&self) -> bool {
        self.shutdown && matches!(self.phase, Phase::Idle | Phase::Asking(_))
    }

    pub fn handle(&mut self, input: Input, now: Instant) -> Vec<Effect> {
        let before = self.state();
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
            Input::Log(line) => self.note(line),
            Input::WorkerBroken(why) => self.lose(Lost::Broken(why), now, &mut effects),
            Input::SpawnFailed(why) => {
                if let Phase::Running(run) = &self.phase {
                    let follows = after(run.restart, Lost::Spawn(why).failure());
                    self.gone(follows, now, &mut effects);
                }
            }
            Input::WorkerExited(exit) => self.exited(exit, now, &mut effects),
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
        if self.state() != before {
            effects.push(Effect::Publish);
        }
        effects
    }

    /// The state the TV app sees.
    #[must_use]
    pub fn state(&self) -> State {
        State {
            package: self.package.published(self.process(), self.reported()),
            preferences: self.preferences.clone(),
            power: self.power,
            video: self.video,
            unreadable: self.unreadable.clone(),
            log: self.log.iter().cloned().collect(),
        }
    }

    /// Where the worker is, with what the person is asked there: also
    /// while the worker that failed stops, and what a running worker asks.
    /// A restart is never asked.
    fn process(&self) -> Process {
        match &self.phase {
            Phase::Idle => Process::Stopped,
            Phase::Running(run) => Process::Running(run.problem()),
            Phase::Stopping(stopping) => Process::Stopping {
                portal: stopping.portal.clone(),
                problem: match &stopping.then {
                    Then::Ask(problem) => Some(problem.clone()),
                    Then::Idle | Then::Restart => None,
                },
            },
            Phase::Asking(problem) => Process::Asking(problem.clone()),
        }
    }

    /// The running worker's report; none without one.
    fn reported(&self) -> Option<&Report> {
        match &self.phase {
            Phase::Running(run) => Some(&run.report),
            Phase::Idle | Phase::Stopping(_) | Phase::Asking(_) => None,
        }
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
            } => Some(
                FullArgs {
                    settings: settings.clone(),
                    listen: *listen,
                    target: *target,
                }
                .to_args(),
            ),
            Package::Slim { enabled } => {
                let prysm = self
                    .preferences
                    .prysm_address
                    .clone()
                    .filter(|_| *enabled)?;
                Some(SlimArgs { prysm }.to_args())
            }
        }
    }

    /// A worker no longer wanted is stopped; one wanted is started from
    /// idle. A stopping worker's exit and an action of the person or the TV
    /// decide the rest.
    fn reconcile(&mut self, now: Instant, effects: &mut Vec<Effect>) {
        match self.phase {
            Phase::Idle => {
                if let Some(arguments) = self.spawn_arguments() {
                    self.spawn(arguments, Restart::Available, now, effects);
                }
            }
            Phase::Running(_) if !self.wanted() => self.end(now, effects),
            Phase::Running(_) | Phase::Stopping(_) | Phase::Asking(_) => {}
        }
    }

    fn spawn(
        &mut self,
        arguments: Vec<String>,
        restart: Restart,
        now: Instant,
        effects: &mut Vec<Effect>,
    ) {
        effects.push(Effect::Spawn(arguments));
        effects.push(Effect::Send(Command::Video {
            video: self.effective_video(),
        }));
        self.phase = Phase::Running(Run {
            heard: now,
            report: Report::new(&self.package),
            restart,
            ended: None,
        });
    }

    /// The restart: a new worker at once, its restart spent (idle while
    /// none is wanted).
    fn respawn(&mut self, now: Instant, effects: &mut Vec<Effect>) {
        self.phase = Phase::Idle;
        if let Some(arguments) = self.spawn_arguments() {
            self.spawn(arguments, Restart::Spent, now, effects);
        }
    }

    /// Stop a running worker; `then` follows its exit. Only a running
    /// worker is stopped.
    fn stop(&mut self, then: Then, now: Instant, effects: &mut Vec<Effect>) {
        self.phase = match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Running(run) => {
                effects.push(Effect::Terminate);
                Phase::Stopping(Stopping {
                    at: now + KILL_AFTER,
                    sent: Sent::Terminate,
                    then,
                    portal: run.report.full().and_then(|status| status.portal.clone()),
                })
            }
            phase @ (Phase::Idle | Phase::Stopping(_) | Phase::Asking(_)) => phase,
        };
    }

    /// The stop the person or the TV asked for (no longer wanted, new
    /// worker settings).
    fn end(&mut self, now: Instant, effects: &mut Vec<Effect>) {
        self.dismiss();
        self.stop(Then::Idle, now, effects);
    }

    /// The person or the TV acted (a lighting choice, capture on or off,
    /// standby, shutdown, new worker settings): what was asked is answered,
    /// a stopping worker's exit is followed by nothing more than a new
    /// worker if one is wanted, and a running one has its restart again.
    /// The one way out of asking.
    fn dismiss(&mut self) {
        match &mut self.phase {
            Phase::Idle => {}
            Phase::Running(run) => run.restart = Restart::Available,
            Phase::Stopping(stopping) => stopping.then = Then::Idle,
            Phase::Asking(_) => self.phase = Phase::Idle,
        }
    }

    /// What follows the exit of a worker stopped as `follows` says.
    fn then(&mut self, follows: After) -> Then {
        match follows {
            After::Restart(why) => {
                self.restarting(&why);
                Then::Restart
            }
            After::GiveUp(problem) | After::Ask(problem) => Then::Ask(problem),
        }
    }

    /// A worker that is gone (exited, never started, let go): a new one at
    /// once, or the person is asked, as `follows` says.
    fn gone(&mut self, follows: After, now: Instant, effects: &mut Vec<Effect>) {
        match follows {
            After::Restart(why) => {
                self.restarting(&why);
                self.respawn(now, effects);
            }
            After::GiveUp(problem) | After::Ask(problem) => self.phase = Phase::Asking(problem),
        }
    }

    /// A restart is never shown as a failure: its cause goes to the tail.
    fn restarting(&mut self, why: &str) {
        self.note(format!("restarting the worker: {why}"));
    }

    /// The supervisor lost a running worker for `lost`: it is stopped. Why
    /// it said its run ends, if it did, wins.
    fn lose(&mut self, lost: Lost, now: Instant, effects: &mut Vec<Effect>) {
        let Phase::Running(run) = &mut self.phase else {
            return;
        };
        let failure = run.ended.take().unwrap_or_else(|| lost.failure());
        let follows = after(run.restart, failure);
        let then = self.then(follows);
        self.stop(then, now, effects);
    }

    /// A line for the state's tail.
    fn note(&mut self, line: String) {
        if self.log.len() == LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(line);
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
            // starts the new one as soon as it is gone.
            self.end(now, effects);
        }
    }

    /// Full: the person's choice ([`Machine::dismiss`]); the worker stays
    /// wanted whatever the target, and a running one is sent it, with the
    /// next sequence number.
    fn set_target(&mut self, target: LightingTarget, effects: &mut Vec<Effect>) {
        let Package::Full {
            target: current, ..
        } = &mut self.package
        else {
            return;
        };
        *current = target;
        self.dismiss();
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

    /// Slim: the person's choice ([`Machine::dismiss`]); off ends the
    /// worker (`reconcile` stops one no longer wanted).
    fn set_enabled(&mut self, enabled: bool) {
        let Package::Slim { enabled: current } = &mut self.package else {
            return;
        };
        *current = enabled;
        self.dismiss();
    }

    fn worker_event(&mut self, event: Event, now: Instant, effects: &mut Vec<Effect>) {
        let run = match &mut self.phase {
            Phase::Running(run) => run,
            // A late line from a worker already being stopped: only its
            // portal is kept (it answers until the worker exits).
            Phase::Stopping(stopping) => {
                if let Event::Status(WorkerStatus::Full(status)) = event {
                    stopping.portal = status.portal;
                }
                return;
            }
            Phase::Idle | Phase::Asking(_) => return,
        };
        run.heard = now;
        match event {
            Event::Status(status) => self.report(status, now, effects),
            Event::Failed(failure) => run.ended = Some(failure),
        }
    }

    /// A running worker's report. Frames give its restart back; a full
    /// worker's failure that a new process recovers stops it, any other
    /// keeps it running.
    fn report(&mut self, status: WorkerStatus, now: Instant, effects: &mut Vec<Effect>) {
        let Phase::Running(run) = &mut self.phase else {
            return;
        };
        let (capturing, target, failure) = match &status {
            WorkerStatus::Full(full) => (
                full.capturing,
                Some((full.target, full.applied)),
                full.failure.clone(),
            ),
            WorkerStatus::Slim(slim) => (slim.capturing, None, None),
        };
        if !run.report.keep(status) {
            let why = "the worker reported the other package's status";
            return self.lose(Lost::Broken(why.into()), now, effects);
        }
        if capturing {
            run.restart = Restart::Available;
        }
        let restart = run.restart;
        if let Some((target, applied)) = target {
            self.follow(target, applied);
        }
        let Some(failure) = failure else {
            return;
        };
        let then = match after(restart, failure) {
            // It keeps running, published from its report ([`Run::problem`]).
            After::Ask(_) => return,
            follows @ (After::Restart(_) | After::GiveUp(_)) => self.then(follows),
        };
        self.stop(then, now, effects);
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

    /// Reaped. A running worker ended for why it said, else for its exit;
    /// a stopping one as its stop decided.
    fn exited(&mut self, exit: Exit, now: Instant, effects: &mut Vec<Effect>) {
        match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Running(run) => {
                let failure = run.ended.unwrap_or_else(|| Lost::Exited(exit).failure());
                self.gone(after(run.restart, failure), now, effects);
            }
            Phase::Stopping(stopping) => match stopping.then {
                // `reconcile` starts the next worker if one is wanted.
                Then::Idle => {}
                Then::Restart => self.respawn(now, effects),
                Then::Ask(problem) => self.phase = Phase::Asking(problem),
            },
            phase @ (Phase::Idle | Phase::Asking(_)) => self.phase = phase,
        }
    }

    /// When [`Input::Tick`] next has something to do: a running worker's
    /// [`SILENCE`], a stopping one's kill or letting go. `None`: only an
    /// input changes anything.
    #[must_use]
    pub fn next_due(&self) -> Option<Instant> {
        match self.phase {
            Phase::Idle | Phase::Asking(_) => None,
            Phase::Running(Run { heard, .. }) => Some(heard + SILENCE),
            Phase::Stopping(Stopping { at, .. }) => Some(at),
        }
    }

    fn tick(&mut self, now: Instant, effects: &mut Vec<Effect>) {
        match &mut self.phase {
            Phase::Running(run) => {
                if now.duration_since(run.heard) >= SILENCE {
                    self.lose(Lost::Silent, now, effects);
                }
            }
            Phase::Stopping(stopping) if now >= stopping.at => match stopping.sent {
                Sent::Terminate => {
                    effects.push(Effect::Kill);
                    stopping.at = now + KILL_AFTER;
                    stopping.sent = Sent::Kill;
                }
                // It may still hold the capture, however it was stopped: the
                // person is asked, whatever its restart.
                Sent::Kill => {
                    effects.push(Effect::Abandon);
                    let follows = after(Restart::Available, Lost::Abandoned.failure());
                    self.gone(follows, now, effects);
                }
            },
            Phase::Idle | Phase::Stopping(_) | Phase::Asking(_) => {}
        }
    }
}

#[cfg(test)]
mod tests;
