//! The supervisor's decisions as a pure state machine: inputs and the current
//! time in, effects out. No I/O, no clock; every rule is table-tested.
use crate::preferences::Preferences;
use crate::state::{Flavor, Phase as PublishedPhase, Power, State};
use prysm_webos_link::{
    Command, Event, FailureKind, FullArgs, LightingTarget, SILENCE, SlimArgs, Video, WorkerStatus,
    exit,
};
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// TERM to KILL. Above the worker's 6 s stop deadline, which is above the
/// capture adapter's 4 s release limit.
pub const KILL_AFTER: Duration = Duration::from_secs(8);
pub const RETRY_MIN: Duration = Duration::from_secs(1);
pub const RETRY_MAX: Duration = Duration::from_secs(30);
/// A worker running this long resets the retry delay. (A failure is cleared
/// earlier, as soon as the worker reports it is up again.)
pub const HEALTHY: Duration = Duration::from_secs(60);
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

impl Spec {
    fn flavor(&self) -> Flavor {
        match self {
            Self::Full { .. } => Flavor::Full,
            Self::Slim => Flavor::Slim,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Running {
        since: Instant,
        heard: Instant,
    },
    Stopping {
        kill_at: Instant,
        killed: bool,
        failed: bool,
    },
    Retrying {
        at: Instant,
    },
}

#[derive(Debug, Clone)]
pub struct Machine {
    spec: Spec,
    preferences: Preferences,
    /// Slim: whether frames should flow. Full: derived from `lighting`.
    enabled: bool,
    /// Full: the engine target, reported by the worker and used at spawn.
    lighting: LightingTarget,
    echo_until: Option<Instant>,
    power: Power,
    video: Video,
    phase: Phase,
    delay: Duration,
    status: Option<WorkerStatus>,
    /// The supervisor's own failure: why the last worker ended or could not start.
    failure: Option<(String, FailureKind)>,
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
            enabled: intent != LightingTarget::Off,
            lighting: match spec {
                Spec::Full { .. } => intent,
                Spec::Slim => LightingTarget::Off,
            },
            spec,
            preferences,
            echo_until: None,
            power: Power::Unknown,
            video: Video::Unknown,
            phase: Phase::Idle,
            delay: RETRY_MIN,
            status: None,
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
        match self.spec {
            Spec::Full { .. } => self.lighting,
            Spec::Slim if self.enabled => LightingTarget::Processed,
            Spec::Slim => LightingTarget::Off,
        }
    }

    /// Shut down and no worker left.
    #[must_use]
    pub fn finished(&self) -> bool {
        self.shutdown && self.phase == Phase::Idle
    }

    pub fn handle(&mut self, input: Input, now: Instant) -> Vec<Effect> {
        let before = self.state();
        let mut effects = Vec::new();
        let video_before = self.effective_video();
        match input {
            Input::Power(power) => self.power = power,
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
                self.fail(format!("worker link broke: {why}"), now, &mut effects)
            }
            Input::SpawnFailed(why) => {
                if self.running() {
                    self.failure = Some((
                        format!("cannot start the worker: {why}"),
                        FailureKind::Other,
                    ));
                    self.retry(now);
                }
            }
            Input::WorkerExited(exit) => self.exited(exit, now),
            Input::Tick => self.tick(now, &mut effects),
            Input::Shutdown => self.shutdown = true,
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
        let flavor = self.spec.flavor();
        let status = self.status.clone().unwrap_or_default();
        let (failure, failure_kind) = match status.failure {
            Some(failure) => (
                Some(failure),
                Some(status.failure_kind.unwrap_or(FailureKind::Other)),
            ),
            None => self
                .failure
                .clone()
                .map_or((None, None), |(failure, kind)| (Some(failure), Some(kind))),
        };
        State {
            flavor,
            preferences: self.preferences.clone(),
            enabled: match flavor {
                Flavor::Full => self.lighting != LightingTarget::Off,
                Flavor::Slim => self.enabled,
            },
            lighting: (flavor == Flavor::Full).then_some(self.lighting),
            power: self.power,
            video: self.video,
            worker: match self.phase {
                Phase::Idle => PublishedPhase::Stopped,
                Phase::Running { .. } => PublishedPhase::Running,
                Phase::Stopping { .. } => PublishedPhase::Stopping,
                Phase::Retrying { .. } => PublishedPhase::Retrying,
            },
            // Only a running worker's report counts: one being stopped (or
            // stuck exiting) is not capturing for the app.
            capturing: self.running() && status.capturing,
            portal: status.portal,
            failure,
            failure_kind,
            encoding: status.encoding,
            log: self.log.iter().cloned().collect(),
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
        if self.shutdown || !self.power.awake() {
            return false;
        }
        match self.spec {
            // The portal stays up while awake, whatever the lighting.
            Spec::Full { .. } => true,
            Spec::Slim => self.enabled && self.preferences.receiver.is_some(),
        }
    }

    fn arguments(&self) -> Option<Vec<String>> {
        match &self.spec {
            Spec::Full { settings, listen } => Some(
                FullArgs {
                    settings: settings.clone(),
                    listen: *listen,
                    lighting: self.lighting,
                }
                .to_args(),
            ),
            Spec::Slim => self.preferences.receiver.map(|receiver| {
                SlimArgs {
                    receiver,
                    capture: self.preferences.capture.unwrap_or_default(),
                }
                .to_args()
            }),
        }
    }

    fn reconcile(&mut self, now: Instant, effects: &mut Vec<Effect>) {
        loop {
            match self.phase {
                Phase::Retrying { at } if !self.wanted() || now >= at => {
                    self.phase = Phase::Idle;
                    continue;
                }
                Phase::Idle if self.wanted() => {
                    if let Some(arguments) = self.arguments() {
                        effects.push(Effect::Spawn(arguments));
                        effects.push(Effect::Send(Command::Video {
                            video: self.effective_video(),
                        }));
                        self.phase = Phase::Running {
                            since: now,
                            heard: now,
                        };
                    }
                }
                Phase::Running { .. } if !self.wanted() => {
                    if self.shutdown || !self.power.awake() || !self.enabled {
                        // A requested stop clears an old failure.
                        self.failure = None;
                    }
                    self.stop(now, false, effects);
                }
                _ => {}
            }
            return;
        }
    }

    fn stop(&mut self, now: Instant, failed: bool, effects: &mut Vec<Effect>) {
        effects.push(Effect::Terminate);
        self.phase = Phase::Stopping {
            kill_at: now + KILL_AFTER,
            killed: false,
            failed,
        };
    }

    fn fail(&mut self, failure: String, now: Instant, effects: &mut Vec<Effect>) {
        if self.running() {
            self.failure = Some((failure, FailureKind::Other));
            self.stop(now, true, effects);
        }
    }

    fn retry(&mut self, now: Instant) {
        self.status = None;
        self.phase = Phase::Retrying {
            at: now + self.delay,
        };
        self.delay = (self.delay * 2).min(RETRY_MAX);
    }

    fn set_preferences(
        &mut self,
        preferences: Preferences,
        now: Instant,
        effects: &mut Vec<Effect>,
    ) {
        let relaunch = self.spec == Spec::Slim
            && (self.preferences.receiver != preferences.receiver
                || self.preferences.capture != preferences.capture);
        self.preferences = preferences;
        if !relaunch {
            return;
        }
        // New worker settings get an immediate attempt with a fresh backoff.
        self.delay = RETRY_MIN;
        match self.phase {
            // A requested restart: no backoff, no failure.
            Phase::Running { .. } => self.stop(now, false, effects),
            Phase::Stopping {
                kill_at, killed, ..
            } => {
                self.phase = Phase::Stopping {
                    kill_at,
                    killed,
                    failed: false,
                };
            }
            // `reconcile` spawns with the new settings.
            Phase::Retrying { .. } => self.phase = Phase::Idle,
            Phase::Idle => {}
        }
    }

    fn set_enabled(&mut self, enabled: bool, now: Instant, effects: &mut Vec<Effect>) {
        match self.spec {
            Spec::Slim => self.enabled = enabled,
            Spec::Full { .. } => {
                self.lighting = match (enabled, self.lighting) {
                    (false, _) => LightingTarget::Off,
                    (true, LightingTarget::Off) => LightingTarget::Processed,
                    (true, current) => current,
                };
                if self.running() {
                    effects.push(Effect::Send(Command::Lighting {
                        target: self.lighting,
                    }));
                    self.echo_until = Some(now + ECHO);
                }
            }
        }
    }

    fn worker_event(&mut self, event: Event, now: Instant, effects: &mut Vec<Effect>) {
        let Phase::Running { since, .. } = self.phase else {
            // A late line from a worker already being stopped.
            if let (Phase::Stopping { .. }, Event::Status(status)) = (self.phase, event) {
                self.status = Some(status);
            }
            return;
        };
        self.phase = Phase::Running { since, heard: now };
        match event {
            Event::Status(status) => {
                // Up again (full: reporting without a failure; slim: frames
                // flowing): the last worker's failure is no longer current.
                let up = status.capturing || (self.spec != Spec::Slim && status.failure.is_none());
                if up {
                    self.failure = None;
                }
                if let Some(target) = status.target {
                    let echoed = self.echo_until.is_none_or(|until| now >= until);
                    if echoed || target == self.lighting {
                        self.lighting = target;
                        self.echo_until = None;
                    }
                }
                self.status = Some(status);
            }
            Event::Faulted { cause } => self.fail(format!("worker faulted: {cause}"), now, effects),
        }
    }

    fn exited(&mut self, exit: Exit, now: Instant) {
        let described = match exit {
            Exit::Code(code) => exit::describe(code),
            Exit::Signal(signal) => format!("worker killed by signal {signal}"),
        };
        let last = self.status.take().and_then(|status| {
            let kind = status.failure_kind.unwrap_or(FailureKind::Other);
            status.failure.map(|failure| (failure, kind))
        });
        match self.phase {
            Phase::Running { .. } => {
                // The worker stops on its own only when something failed.
                self.failure = Some(match last {
                    Some((last, kind)) => (format!("{described}: {last}"), kind),
                    None => (described, FailureKind::Other),
                });
                self.retry(now);
            }
            Phase::Stopping { failed: true, .. } => self.retry(now),
            Phase::Stopping { failed: false, .. } => self.phase = Phase::Idle,
            Phase::Idle | Phase::Retrying { .. } => {}
        }
    }

    fn tick(&mut self, now: Instant, effects: &mut Vec<Effect>) {
        match self.phase {
            Phase::Running { since, heard } => {
                if now.duration_since(since) >= HEALTHY {
                    self.delay = RETRY_MIN;
                    self.failure = None;
                }
                if now.duration_since(heard) >= SILENCE {
                    self.fail(
                        format!("worker sent nothing for {} s", SILENCE.as_secs()),
                        now,
                        effects,
                    );
                }
            }
            Phase::Stopping {
                kill_at,
                killed: false,
                failed,
            } if now >= kill_at => {
                effects.push(Effect::Kill);
                self.phase = Phase::Stopping {
                    kill_at,
                    killed: true,
                    failed,
                };
            }
            _ => {}
        }
        if self.echo_until.is_some_and(|until| now >= until) {
            self.echo_until = None;
        }
    }
}

#[cfg(test)]
mod tests;
