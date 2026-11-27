//! The supervisor's decisions as a pure state machine: inputs and the current
//! time in, effects out.
//!
//! - [`phase`]: where the worker is, with what is valid only there.
//! - [`recovery`]: what follows a failed worker, the one reading of an
//!   escalation and its one automatic recovery, a restart.
use crate::preferences::Preferences;
use crate::state::{self, Power, State};
use prysm_webos_link::{
    Command, Event, FullArgs, KILL_AFTER, LightingTarget, PrysmAddress, SILENCE, SlimArgs, Video,
    WorkerStatus,
};
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Instant;

mod phase;
mod recovery;
use phase::{Phase, Report, Run, Sent, Stopping, Then};
use recovery::{After, WorkerStage, after, fail_fast};

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
    /// Full only: request the engine's remembered mode.
    Resume,
    /// Slim only: frames should flow. Full ignores it (the service refuses
    /// it first).
    Enabled(bool),
    Worker(Event),
    /// Whether the supervisor is receiving video notifications.
    VideoObservation(bool),
    /// A line for the state's tail: the supervisor's or the worker's log.
    Log(String),
    /// The worker's event stream broke (bad line, oversized, truncated).
    WorkerBroken(String),
    SpawnFailed(String),
    /// Reaped: the process no longer exists.
    WorkerExited(Exit),
    /// The worker let go ([`Effect::Abandon`]) was confirmed gone.
    LetGoReaped,
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
    /// may still hold the capture: the person is asked to restart the TV
    /// until it no longer exists ([`Input::LetGoReaped`]).
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
        args: FullArgs,
        /// The target last requested or reported.
        target: LightingTarget,
        /// An explicit user request still to dispatch.
        target_pending: Option<TargetRequest>,
    },
    Slim {
        /// Whether frames should flow.
        enabled: bool,
    },
}

enum WantedWorker<'a> {
    Full(&'a FullArgs),
    Slim(&'a PrysmAddress),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetRequest {
    Set(LightingTarget),
    Remembered,
    RecoverSync,
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
    observes_video: bool,
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
                    args: FullArgs { settings, listen },
                    target: intent,
                    target_pending: None,
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
            observes_video: false,
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
        self.shutdown
            && matches!(
                self.phase,
                Phase::Idle | Phase::Asking(_) | Phase::LetGo(..)
            )
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
            Input::Target(target) => self.set_target(TargetRequest::Set(target), &mut effects),
            Input::Resume => self.set_target(TargetRequest::Remembered, &mut effects),
            Input::Enabled(enabled) => self.set_enabled(enabled),
            Input::Worker(event) => self.worker_event(event, now, &mut effects),
            Input::VideoObservation(available) if available != self.observes_video => {
                self.observes_video = available;
                if self.running() {
                    effects.push(Effect::Send(Command::ObserveVideo { available }));
                }
            }
            Input::VideoObservation(_) => {}
            Input::Log(line) => self.note(line),
            Input::WorkerBroken(why) => self.broken_link(why, now, &mut effects),
            Input::SpawnFailed(why) => {
                if let Phase::Running(run) = &self.phase {
                    let follows = after(
                        run.stage,
                        fail_fast(format!("cannot start the worker: {why}")),
                    );
                    self.gone(follows, now, &mut effects);
                }
            }
            Input::WorkerExited(exit) => self.exited(exit, now, &mut effects),
            Input::LetGoReaped => {
                if let Phase::LetGo(then) = &mut self.phase {
                    let then = std::mem::replace(then, Then::Idle);
                    self.phase = Phase::Idle;
                    self.apply_exit_action(then, now, &mut effects);
                }
            }
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
            package: match self.package {
                Package::Full { target, .. } => state::Package::Full {
                    target,
                    worker: self.phase.full_worker(),
                },
                Package::Slim { enabled } => state::Package::Slim {
                    enabled,
                    worker: self.phase.slim_worker(),
                },
            },
            preferences: self.preferences.clone(),
            power: self.power,
            video: self.video,
            unreadable: self.unreadable.clone(),
            log: self.log.iter().cloned().collect(),
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

    fn wanted_worker(&self) -> Option<WantedWorker<'_>> {
        if self.shutdown || !self.power.awake() {
            return None;
        }
        match &self.package {
            Package::Full { args, .. } => Some(WantedWorker::Full(args)),
            Package::Slim { enabled: true } => self
                .preferences
                .prysm_address
                .as_ref()
                .map(WantedWorker::Slim),
            Package::Slim { enabled: false } => None,
        }
    }

    /// A worker no longer wanted is stopped; one wanted is started from
    /// idle. A stopping worker's exit and an action of the person or the TV
    /// decide the rest.
    fn reconcile(&mut self, now: Instant, effects: &mut Vec<Effect>) {
        match self.phase {
            Phase::Idle => self.spawn(WorkerStage::Starting, now, effects),
            Phase::Running(_) if self.wanted_worker().is_none() => self.end(now, effects),
            Phase::Running(_) | Phase::Stopping(_) | Phase::Asking(_) | Phase::LetGo(..) => {}
        }
    }

    fn spawn(&mut self, stage: WorkerStage, now: Instant, effects: &mut Vec<Effect>) {
        let Some(worker) = self.wanted_worker() else {
            self.phase = Phase::Idle;
            return;
        };
        let arguments = match worker {
            WantedWorker::Full(args) => args.to_args(),
            WantedWorker::Slim(prysm) => SlimArgs {
                prysm: prysm.clone(),
            }
            .to_args(),
        };
        if stage == WorkerStage::Restarting
            && let Package::Full {
                target: LightingTarget::ScreenSync,
                target_pending,
                ..
            } = &mut self.package
        {
            target_pending.get_or_insert(TargetRequest::RecoverSync);
        }
        effects.push(Effect::Spawn(arguments));
        self.phase = Phase::Running(Run {
            heard: now,
            report: Report::new(&self.package),
            stage,
            ended: None,
        });
        self.send_target(effects);
        if self.observes_video {
            effects.push(Effect::Send(Command::ObserveVideo { available: true }));
        }
        effects.push(Effect::Send(Command::Video {
            video: self.effective_video(),
        }));
    }

    /// Stop a running worker; `then` follows its exit. Only a running
    /// worker is stopped.
    fn stop(&mut self, then: Then, now: Instant, effects: &mut Vec<Effect>) {
        if let Phase::Running(run) = &self.phase {
            effects.push(Effect::Terminate);
            self.phase = Phase::Stopping(Stopping {
                at: now + KILL_AFTER,
                sent: Sent::Terminate,
                then,
                portal: run.report.full().and_then(|status| status.portal.clone()),
            });
        }
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
    /// The one way out of asking, except about a worker let go: what was
    /// chosen is kept, and applies once it no longer exists.
    fn dismiss(&mut self) {
        match &mut self.phase {
            Phase::LetGo(then) => *then = Then::Idle,
            Phase::Running(run) if run.stage == WorkerStage::Restarting => {
                run.stage = WorkerStage::Starting;
            }
            Phase::Idle | Phase::Running(_) => {}
            Phase::Stopping(stopping) => stopping.then = Then::Idle,
            Phase::Asking(_) => self.phase = Phase::Idle,
        }
    }

    /// What follows the exit of a worker stopped as `follows` says.
    fn exit_action(&mut self, follows: After) -> Then {
        match follows {
            After::Restart(why) => {
                self.note(format!("restarting the worker: {why}"));
                Then::Restart
            }
            After::GiveUp(problem) | After::Ask(problem) => Then::Ask(problem),
        }
    }

    /// A worker that is gone (exited, never started): a new one at once, or
    /// the person is asked, as `follows` says.
    fn gone(&mut self, follows: After, now: Instant, effects: &mut Vec<Effect>) {
        let then = self.exit_action(follows);
        self.apply_exit_action(then, now, effects);
    }

    fn broken_link(&mut self, why: String, now: Instant, effects: &mut Vec<Effect>) {
        let Phase::Running(run) = &self.phase else {
            return;
        };
        let failure = run
            .ended
            .clone()
            .unwrap_or_else(|| fail_fast(format!("worker link broke: {why}")));
        let follows = after(run.stage, failure);
        let then = self.exit_action(follows);
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
    fn set_target(&mut self, request: TargetRequest, effects: &mut Vec<Effect>) {
        let Package::Full {
            target,
            target_pending,
            ..
        } = &mut self.package
        else {
            return;
        };
        if let TargetRequest::Set(requested) = request {
            *target = requested;
        }
        *target_pending = Some(request);
        self.dismiss();
        self.send_target(effects);
    }

    fn send_target(&mut self, effects: &mut Vec<Effect>) {
        if let Package::Full { target_pending, .. } = &mut self.package
            && let Phase::Running(Run {
                report: Report::Full { sent, .. },
                ..
            }) = &mut self.phase
            && let Some(request) = target_pending.take()
        {
            *sent += 1;
            let command = match request {
                TargetRequest::Set(target) => Command::Target {
                    target,
                    sequence: *sent,
                },
                TargetRequest::Remembered => Command::Resume { sequence: *sent },
                TargetRequest::RecoverSync => Command::RecoverScreenSync { sequence: *sent },
            };
            effects.push(Effect::Send(command));
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
            Phase::Idle | Phase::Asking(_) | Phase::LetGo(..) => return,
        };
        run.heard = now;
        match event {
            Event::Status(status) => self.report(status, now, effects),
            Event::Initialized {} if run.ended.is_none() => run.stage = WorkerStage::Stable,
            Event::Initialized {} => {}
            Event::Failed(failure) => {
                if run.ended.is_none() {
                    run.ended = Some(failure);
                }
            }
        }
    }

    /// A full worker's failure that requires a new process stops it.
    /// Other reports leave the worker running.
    fn report(&mut self, status: WorkerStatus, now: Instant, effects: &mut Vec<Effect>) {
        let Phase::Running(run) = &mut self.phase else {
            return;
        };
        if run.ended.is_some() {
            return;
        }
        if !run.report.keep(status) {
            let why = "the worker reported the other package's status";
            return self.broken_link(why.into(), now, effects);
        }
        let Some(current) = run.report.current_full() else {
            return;
        };
        if let Package::Full { target, .. } = &mut self.package {
            *target = current.target;
        }
        let Some(failure) = current.failure.clone() else {
            return;
        };
        let then = match after(run.stage, failure) {
            // It keeps running, published from its report ([`Run::problem`]).
            After::Ask(_) => return,
            follows @ (After::Restart(_) | After::GiveUp(_)) => self.exit_action(follows),
        };
        self.stop(then, now, effects);
    }

    /// Reaped. A running worker ended for why it said, else for its exit;
    /// a stopping one as its stop decided.
    fn exited(&mut self, exit: Exit, now: Instant, effects: &mut Vec<Effect>) {
        match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Running(run) => {
                let message = match exit {
                    Exit::Code(code) => format!("worker exited with code {code}"),
                    Exit::Signal(signal) => format!("worker killed by signal {signal}"),
                };
                let follows = run.after_disconnect(message);
                self.gone(follows, now, effects);
            }
            Phase::Stopping(stopping) => self.apply_exit_action(stopping.then, now, effects),
            phase @ (Phase::Idle | Phase::Asking(_) | Phase::LetGo(..)) => self.phase = phase,
        }
    }

    fn apply_exit_action(&mut self, then: Then, now: Instant, effects: &mut Vec<Effect>) {
        match then {
            Then::Idle => {}
            Then::Restart => self.spawn(WorkerStage::Restarting, now, effects),
            Then::Ask(problem) => self.phase = Phase::Asking(problem),
        }
    }

    /// When [`Input::Tick`] next has something to do: a running worker's
    /// [`SILENCE`], a stopping one's kill or letting go. `None`: only an
    /// input changes anything.
    #[must_use]
    pub fn next_due(&self) -> Option<Instant> {
        match self.phase {
            Phase::Idle | Phase::Asking(_) | Phase::LetGo(..) => None,
            Phase::Running(Run { heard, .. }) => Some(heard + SILENCE),
            Phase::Stopping(Stopping { at, .. }) => Some(at),
        }
    }

    fn tick(&mut self, now: Instant, effects: &mut Vec<Effect>) {
        match &mut self.phase {
            Phase::Running(run) => {
                if now.duration_since(run.heard) >= SILENCE {
                    let follows = run.after_disconnect(format!(
                        "worker sent nothing for {} s",
                        SILENCE.as_secs()
                    ));
                    let then = self.exit_action(follows);
                    self.stop(then, now, effects);
                }
            }
            Phase::Stopping(stopping) if now >= stopping.at => match stopping.sent {
                Sent::Terminate => {
                    effects.push(Effect::Kill);
                    stopping.at = now + KILL_AFTER;
                    stopping.sent = Sent::Kill;
                }
                Sent::Kill => {
                    effects.push(Effect::Abandon);
                    let then = std::mem::replace(&mut stopping.then, Then::Idle);
                    self.phase = Phase::LetGo(then);
                }
            },
            Phase::Idle | Phase::Stopping(_) | Phase::Asking(_) | Phase::LetGo(..) => {}
        }
    }
}

#[cfg(test)]
mod tests;
