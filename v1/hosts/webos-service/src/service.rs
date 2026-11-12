//! The driver: Luna requests and subscription replies in, effects of the
//! [`Machine`] carried out on the worker, the bus and the files. Everything
//! native sits behind [`Bus`], [`Launcher`], [`PreferencesStore`] and
//! [`Installer`], so this runs unchanged against fakes.
use crate::install::Installer;
use crate::launcher::{Launcher, Output, Worker};
use crate::machine::{Effect, Input, Machine, Spec};
use crate::observe;
use crate::preferences::{Preferences, PreferencesStore};
use crate::state::{Flavor, Power, State};
use prysm_webos_link::{LightingTarget, Video};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Largest accepted request payload.
pub const MAX_PAYLOAD: usize = 4096;
/// A subscription without a good reply for this long is renewed.
pub const RESUBSCRIBE: Duration = Duration::from_secs(5);

/// A Luna method this service registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    GetState,
    SetPreferences,
    SetEnabled,
    Uninstall,
}

impl Method {
    pub const ALL: [Self; 4] = [
        Self::GetState,
        Self::SetPreferences,
        Self::SetEnabled,
        Self::Uninstall,
    ];

    /// The method's Luna name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::GetState => "getState",
            Self::SetPreferences => "setPreferences",
            Self::SetEnabled => "setEnabled",
            Self::Uninstall => "uninstall",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|method| method.name() == name)
    }
}

/// The Luna names of [`Method::ALL`], in registration order.
pub const METHODS: [&str; 4] = [
    Method::ALL[0].name(),
    Method::ALL[1].name(),
    Method::ALL[2].name(),
    Method::ALL[3].name(),
];
/// Subscription key of `getState`.
pub const STATE_KEY: &str = "/getState";

/// The two subscriptions the service keeps, in call order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Watch {
    Power = 0,
    Video = 1,
}

/// `(uri, payload)` of each [`Watch`], in order.
pub const WATCHES: [(&str, &str); 2] = [
    (
        "luna://com.webos.service.tvpower/power/getPowerState",
        r#"{"subscribe":true}"#,
    ),
    (
        "luna://com.webos.service.videooutput/getStatus",
        r#"{"subscribe":true}"#,
    ),
];

/// The service's way out to Luna.
pub trait Bus {
    /// Send `payload` to every `getState` subscriber.
    fn publish(&mut self, payload: &str);
    /// Cancel and repeat a subscription call.
    fn resubscribe(&mut self, watch: Watch);
}

/// A reply to one request; `subscribe` asks the bus to add the caller to
/// the `getState` subscribers before replying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub payload: String,
    pub subscribe: bool,
}

impl Reply {
    fn error(text: impl Into<String>) -> Self {
        let mut reply = Map::new();
        reply.insert("returnValue".into(), false.into());
        reply.insert("errorText".into(), text.into().into());
        Self {
            payload: Value::Object(reply).to_string(),
            subscribe: false,
        }
    }
}

/// Fixed facts of one service instance.
#[derive(Debug, Clone)]
pub struct Config {
    pub flavor: Flavor,
    /// The only Luna caller allowed to change anything.
    pub app_id: String,
    /// The worker binary.
    pub program: PathBuf,
    pub spec: Spec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Uninstall {
    No,
    Requested,
}

pub struct Service<L: Launcher, S: PreferencesStore, I: Installer> {
    config: Config,
    machine: Machine,
    launcher: L,
    store: S,
    installer: I,
    worker: Option<L::Worker>,
    retry: [Option<Instant>; 2],
    video_context: Option<Value>,
    published: Option<String>,
    /// The intent last given to the store.
    remembered: Option<LightingTarget>,
    uninstall: Uninstall,
}

impl<L: Launcher, S: PreferencesStore, I: Installer> Service<L, S, I> {
    /// Load preferences (invalid ones are reported and replaced by defaults).
    pub fn new(config: Config, launcher: L, mut store: S, installer: I, now: Instant) -> Self {
        let defaults = Preferences::defaults(config.flavor);
        let preferences = match store.load() {
            Ok(Some(preferences)) => match preferences.validate(config.flavor) {
                Ok(()) => preferences.for_flavor(config.flavor),
                Err(error) => {
                    eprintln!("prysm-webos-service: ignoring saved preferences: {error}");
                    defaults
                }
            },
            Ok(None) => defaults,
            Err(error) => {
                eprintln!("prysm-webos-service: ignoring saved preferences: {error}");
                defaults
            }
        };
        // A restart within one boot (an update, a crash) resumes the intent.
        let resumed = store.resume();
        let machine = match resumed {
            Some(intent) => Machine::resumed(config.spec.clone(), preferences, intent),
            None => Machine::new(config.spec.clone(), preferences),
        };
        Self {
            config,
            machine,
            launcher,
            store,
            installer,
            worker: None,
            retry: [Some(now + RESUBSCRIBE); 2],
            video_context: None,
            published: None,
            remembered: resumed,
            uninstall: Uninstall::No,
        }
    }

    #[must_use]
    pub fn machine(&self) -> &Machine {
        &self.machine
    }

    /// One Luna method call. `sender` is LS2's application id (or service
    /// name) of the caller, never anything from the payload.
    pub fn request(
        &mut self,
        bus: &mut impl Bus,
        method: &str,
        sender: &str,
        payload: &str,
        now: Instant,
    ) -> Reply {
        match self.try_request(bus, method, sender, payload, now) {
            Ok(reply) => reply,
            Err(error) => Reply::error(error),
        }
    }

    fn try_request(
        &mut self,
        bus: &mut impl Bus,
        method: &str,
        sender: &str,
        payload: &str,
        now: Instant,
    ) -> Result<Reply, String> {
        if payload.len() > MAX_PAYLOAD {
            return Err(format!("payload exceeds {MAX_PAYLOAD} bytes"));
        }
        let Some(method) = Method::parse(method) else {
            return Err(format!("unknown method `{method}`"));
        };
        if method != Method::GetState && sender != self.config.app_id {
            // Checked before the payload is even parsed.
            return Err(format!("only {} may change Prysm", self.config.app_id));
        }
        if self.uninstall == Uninstall::Requested {
            return Err("the service is being removed".into());
        }
        let name = method.name();
        let parse_error = |error: serde_json::Error| format!("invalid {name} request: {error}");
        // Only objects: serde would also accept a struct as an array.
        let payload = match serde_json::from_str::<Value>(payload).map_err(parse_error)? {
            Value::Object(object) => Value::Object(object),
            _ => return Err(format!("invalid {name} request: expected an object")),
        };
        match method {
            Method::GetState => {
                let request: GetStateRequest =
                    serde_json::from_value(payload).map_err(parse_error)?;
                Ok(Reply {
                    payload: self.state_json(Some(request.subscribe)),
                    subscribe: request.subscribe,
                })
            }
            Method::SetPreferences => {
                let preferences: Preferences =
                    serde_json::from_value(payload).map_err(parse_error)?;
                preferences.validate(self.config.flavor)?;
                let preferences = preferences.for_flavor(self.config.flavor);
                self.store.save(&preferences)?;
                self.input(bus, Input::Preferences(preferences), now);
                Ok(self.state_reply())
            }
            Method::SetEnabled => {
                let request: SetEnabledRequest =
                    serde_json::from_value(payload).map_err(parse_error)?;
                self.input(bus, Input::Enabled(request.enabled), now);
                Ok(self.state_reply())
            }
            Method::Uninstall => {
                let UninstallRequest {} = serde_json::from_value(payload).map_err(parse_error)?;
                self.uninstall = Uninstall::Requested;
                self.input(bus, Input::Shutdown, now);
                Ok(Reply {
                    payload: r#"{"returnValue":true}"#.into(),
                    subscribe: false,
                })
            }
        }
    }

    /// A `tvpower` subscription reply.
    pub fn power(&mut self, bus: &mut impl Bus, payload: &str, now: Instant) {
        let reading = serde_json::from_str(payload)
            .ok()
            .and_then(|value: Value| observe::power(&value));
        let slot = &mut self.retry[Watch::Power as usize];
        if reading.is_some() {
            *slot = None;
        } else {
            slot.get_or_insert(now + RESUBSCRIBE);
        }
        self.input(bus, Input::Power(reading.unwrap_or(Power::Unknown)), now);
    }

    /// A `videooutput` subscription reply.
    pub fn video(&mut self, bus: &mut impl Bus, payload: &str, now: Instant) {
        let value = serde_json::from_str(payload).unwrap_or(Value::Null);
        let reading = observe::video(&value);
        let slot = &mut self.retry[Watch::Video as usize];
        if reading.settled {
            *slot = None;
        } else {
            slot.get_or_insert(now + RESUBSCRIBE);
        }
        if let Some(context) = reading.context {
            let changed = self
                .video_context
                .as_ref()
                .is_some_and(|previous| *previous != context);
            self.video_context = Some(context);
            if changed {
                self.input(bus, Input::VideoContextChanged, now);
            }
        }
        self.input(bus, Input::Video(reading.video), now);
    }

    /// Stop the worker and then the service.
    pub fn shutdown(&mut self, bus: &mut impl Bus, now: Instant) {
        self.input(bus, Input::Shutdown, now);
    }

    /// Called about every 100 ms. Returns `false` once the service should exit.
    pub fn tick(&mut self, bus: &mut impl Bus, now: Instant) -> bool {
        self.poll_worker(bus, now);
        self.input(bus, Input::Tick, now);
        for watch in [Watch::Power, Watch::Video] {
            let slot = &mut self.retry[watch as usize];
            if slot.is_some_and(|at| now >= at) {
                *slot = Some(now + RESUBSCRIBE);
                bus.resubscribe(watch);
                if watch == Watch::Video {
                    // Unknown video still lets capture try, bounded by the adapter.
                    self.input(bus, Input::Video(Video::Unknown), now);
                }
            }
        }
        if !self.machine.finished() {
            return true;
        }
        if self.uninstall == Uninstall::Requested
            && let Err(error) = self.installer.uninstall()
        {
            eprintln!("prysm-webos-service: uninstall failed: {error}");
        }
        false
    }

    fn poll_worker(&mut self, bus: &mut impl Bus, now: Instant) {
        let Some(worker) = &mut self.worker else {
            return;
        };
        let mut outputs = worker.drain();
        let exit = match worker.try_exit() {
            Ok(exit) => exit,
            Err(error) => {
                eprintln!("prysm-webos-service: cannot wait for the worker: {error}");
                None
            }
        };
        if exit.is_some() {
            // Lines written just before exiting.
            outputs.extend(worker.drain());
            self.worker = None;
        }
        for output in outputs {
            let input = match output {
                Output::Event(event) => Input::Worker(event),
                Output::Log(line) => Input::WorkerLog(line),
                Output::Broken(why) => Input::WorkerBroken(why),
            };
            self.input(bus, input, now);
        }
        if let Some(exit) = exit {
            self.input(bus, Input::WorkerExited(exit), now);
        }
    }

    fn input(&mut self, bus: &mut impl Bus, input: Input, now: Instant) {
        let effects = self.machine.handle(input, now);
        self.execute(bus, effects, now);
        let intent = self.machine.intent();
        if self.remembered != Some(intent) {
            // Best effort, reported once per change.
            self.remembered = Some(intent);
            if let Err(error) = self.store.remember(intent) {
                eprintln!("prysm-webos-service: {error}");
            }
        }
    }

    fn execute(&mut self, bus: &mut impl Bus, effects: Vec<Effect>, now: Instant) {
        let mut failed_spawn = None;
        for effect in effects {
            match effect {
                Effect::Spawn(arguments) => {
                    debug_assert!(self.worker.is_none(), "spawn before reap");
                    match self.launcher.spawn(&self.config.program, &arguments) {
                        Ok(worker) => self.worker = Some(worker),
                        Err(error) => failed_spawn = Some(error.to_string()),
                    }
                }
                Effect::Send(command) => {
                    if let Some(worker) = &mut self.worker {
                        // A worker that stopped reading is exiting; its exit follows.
                        let _ = worker.send(&command);
                    }
                }
                Effect::Terminate => {
                    if let Some(worker) = &mut self.worker {
                        let _ = worker.terminate();
                    }
                }
                Effect::Kill => {
                    if let Some(worker) = &mut self.worker {
                        let _ = worker.kill();
                    }
                }
                Effect::Publish => {
                    let payload = self.state_json(Some(true));
                    if self.published.as_ref() != Some(&payload) {
                        bus.publish(&payload);
                        self.published = Some(payload);
                    }
                }
            }
        }
        if let Some(why) = failed_spawn {
            self.input(bus, Input::SpawnFailed(why), now);
        }
    }

    fn state_reply(&self) -> Reply {
        Reply {
            payload: self.state_json(None),
            subscribe: false,
        }
    }

    fn state_json(&self, subscribed: Option<bool>) -> String {
        state_reply(&self.machine.state(), subscribed).to_string()
    }
}

/// The `getState` reply and subscription payload: [`State`] plus LS2's
/// `returnValue` and, on `getState` and subscription updates, `subscribed`.
#[must_use]
pub fn state_reply(state: &State, subscribed: Option<bool>) -> Value {
    let mut value = match serde_json::to_value(state) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };
    value.insert("returnValue".into(), true.into());
    if let Some(subscribed) = subscribed {
        value.insert("subscribed".into(), subscribed.into());
    }
    Value::Object(value)
}

/// `getState` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct GetStateRequest {
    /// Keep sending the state on every change.
    #[serde(default)]
    pub subscribe: bool,
}

/// `setEnabled` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct SetEnabledRequest {
    /// Slim: frames should flow. Full: processed lighting (or off).
    pub enabled: bool,
}

/// `uninstall` request: an empty object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct UninstallRequest {}

#[cfg(test)]
pub(crate) mod tests;
