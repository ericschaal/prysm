//! The supervisor's decisions as a pure state machine: inputs and the current
//! time in, effects out. No I/O, no clock; every rule is table-tested.
use crate::preferences::Preferences;
use crate::state::{Flavor, Phase as PublishedPhase, Power, State};
use prysm_webos_link::{
    Backoff, Command, Event, FullArgs, LightingTarget, SILENCE, SlimArgs, Video, WorkerFailure,
    WorkerStatus, exit,
};
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// TERM to KILL. Above the worker's 6 s stop deadline, which is above the
/// capture adapter's 4 s release limit.
pub const KILL_AFTER: Duration = Duration::from_secs(8);
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

/// The package's worker and what the user asked of it, in its terms.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Package {
    Full {
        settings: PathBuf,
        listen: SocketAddr,
        /// The engine target, reported by the worker and used at spawn.
        lighting: LightingTarget,
    },
    Slim {
        /// Whether frames should flow.
        enabled: bool,
    },
}

#[derive(Debug, Clone)]
pub struct Machine {
    package: Package,
    preferences: Preferences,
    echo_until: Option<Instant>,
    power: Power,
    video: Video,
    phase: Phase,
    /// Restart delays: a worker that ran [`Backoff::HEALTHY`] resets them. (A
    /// failure is cleared earlier, as soon as the worker reports it is up again.)
    backoff: Backoff,
    status: Option<WorkerStatus>,
    /// The supervisor's own failure: why the last worker ended or could not start.
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
                },
                Spec::Slim => Package::Slim {
                    enabled: intent != LightingTarget::Off,
                },
            },
            preferences,
            echo_until: None,
            power: Power::Unknown,
            video: Video::Unknown,
            phase: Phase::Idle,
            backoff: Backoff::default(),
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
        match self.package {
            Package::Full { lighting, .. } => lighting,
            Package::Slim { enabled: true } => LightingTarget::Processed,
            Package::Slim { enabled: false } => LightingTarget::Off,
        }
    }

    /// Slim: frames should flow. Full: lighting is not off.
    fn enabled(&self) -> bool {
        match self.package {
            Package::Full { lighting, .. } => lighting != LightingTarget::Off,
            Package::Slim { enabled } => enabled,
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
                    self.failure = Some(WorkerFailure::other(format!(
                        "cannot start the worker: {why}"
                    )));
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
        let status = self.status.clone().unwrap_or_default();
        // The worker's own report wins over why the last one ended.
        let failure = status.failure.or_else(|| self.failure.clone());
        let (flavor, lighting) = match self.package {
            Package::Full { lighting, .. } => (Flavor::Full, Some(lighting)),
            Package::Slim { .. } => (Flavor::Slim, None),
        };
        State {
            flavor,
            preferences: self.preferences.clone(),
            enabled: self.enabled(),
            lighting,
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
            failure_kind: failure.as_ref().map(|failure| failure.kind),
            failure: failure.map(|failure| failure.message),
            encoding: status.encoding,
            paused: self.running() && status.paused,
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
        match self.package {
            // The portal stays up while awake, whatever the lighting.
            Package::Full { .. } => true,
            Package::Slim { enabled } => enabled && self.preferences.receiver.is_some(),
        }
    }

    fn arguments(&self) -> Option<Vec<String>> {
        match &self.package {
            Package::Full {
                settings,
                listen,
                lighting,
            } => Some(
                FullArgs {
                    settings: settings.clone(),
                    listen: *listen,
                    lighting: *lighting,
                }
                .to_args(),
            ),
            Package::Slim { .. } => self
                .preferences
                .receiver
                .clone()
                .map(|receiver| SlimArgs { receiver }.to_args()),
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
                    if self.shutdown || !self.power.awake() || !self.enabled() {
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
            self.failure = Some(WorkerFailure::other(failure));
            self.stop(now, true, effects);
        }
    }

    fn retry(&mut self, now: Instant) {
        self.status = None;
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
            && self.preferences.receiver != preferences.receiver;
        self.preferences = preferences;
        if !relaunch {
            return;
        }
        // New worker settings get an immediate attempt with a fresh backoff.
        self.backoff.reset();
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
        let running = self.running();
        match &mut self.package {
            Package::Slim { enabled: current } => *current = enabled,
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
                // flowing, or paused by a receiver it reached): the last
                // worker's failure is no longer current.
                let full = matches!(self.package, Package::Full { .. });
                let up = status.capturing || status.paused || (full && status.failure.is_none());
                if up {
                    self.failure = None;
                }
                if let (Some(target), Package::Full { lighting, .. }) =
                    (status.target, &mut self.package)
                {
                    let echoed = self.echo_until.is_none_or(|until| now >= until);
                    if echoed || target == *lighting {
                        *lighting = target;
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
        let last = self.status.take().and_then(|status| status.failure);
        match self.phase {
            Phase::Running { .. } => {
                // The worker stops on its own only when something failed.
                self.failure = Some(match last {
                    Some(last) => WorkerFailure {
                        message: format!("{described}: {}", last.message),
                        ..last
                    },
                    None => WorkerFailure::other(described),
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
                if now.duration_since(since) >= Backoff::HEALTHY {
                    self.backoff.reset();
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
