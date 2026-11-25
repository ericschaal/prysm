//! The driver: Luna requests and subscription replies in, effects of the
//! [`Machine`] carried out on the worker, the bus and the files. Everything
//! native sits behind [`Bus`], [`Launcher`], [`PreferencesStore`] and
//! [`Installer`], so this runs unchanged against fakes.
use crate::install::Installer;
use crate::launcher::{Launcher, Output, Worker};
use crate::machine::{Effect, Input, Machine, Spec};
use crate::observe;
use crate::preferences::{Preferences, PreferencesFile, PreferencesStore};
use crate::state::{Flavor, Power, State};
use prysm_ports::chain;
use prysm_webos_link::LightingTarget;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tracing::{debug, error, warn};

/// Largest accepted request payload.
pub const MAX_PAYLOAD: usize = 4096;
/// Delay before a subscription that is not live is made again: at start
/// until its first good reply, and after an error reply or one that is not
/// subscribed. The TV's service may not be registered yet (boot) or may have
/// restarted, and nothing else brings a failed subscription back. A live
/// one is left alone: an hour on a TV showed it reports every change by
/// itself, however long the gap.
pub const RESUBSCRIBE: Duration = Duration::from_secs(5);

/// A Luna method this service registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    GetState,
    SetPreferences,
    /// Full only.
    SetTarget,
    /// Slim only.
    SetEnabled,
    Uninstall,
}

impl Method {
    pub const ALL: [Self; 5] = [
        Self::GetState,
        Self::SetPreferences,
        Self::SetTarget,
        Self::SetEnabled,
        Self::Uninstall,
    ];

    /// The method's Luna name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::GetState => "getState",
            Self::SetPreferences => "setPreferences",
            Self::SetTarget => "setTarget",
            Self::SetEnabled => "setEnabled",
            Self::Uninstall => "uninstall",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|method| method.name() == name)
    }
}

/// The Luna names of [`Method::ALL`], in registration order.
pub const METHODS: [&str; 5] = [
    Method::ALL[0].name(),
    Method::ALL[1].name(),
    Method::ALL[2].name(),
    Method::ALL[3].name(),
    Method::ALL[4].name(),
];
/// Subscription key of `getState`.
pub const STATE_KEY: &str = "/getState";

/// The two subscriptions the service keeps, in call order: the
/// discriminant is the call's index in [`WATCHES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Watch {
    Power = 0,
    Video = 1,
}

/// When each subscription is renewed, unless it became live first.
#[derive(Debug, Clone, Copy)]
struct Renewals {
    power: Option<Instant>,
    video: Option<Instant>,
}

impl Renewals {
    fn of(&mut self, watch: Watch) -> &mut Option<Instant> {
        match watch {
            Watch::Power => &mut self.power,
            Watch::Video => &mut self.video,
        }
    }
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

/// Why a call failed: the error reply's `errorKind`. `too_large`: the
/// payload exceeds 4096 bytes. `unknown_method`: no such method.
/// `forbidden`: only the TV app may change Prysm. `removing`: the service is
/// being removed. `invalid_request`: the payload is not the method's request.
/// `wrong_package`: the other package's method (`setTarget` on slim,
/// `setEnabled` on full). `invalid_preferences`: a preference the installed
/// package does not have. `save_failed`: the preferences could not be saved.
/// `internal`: the service could not read the call or send its reply. Open:
/// an app reads a kind it does not know as a refusal and shows `errorText`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum ErrorKind {
    /// The payload exceeds [`MAX_PAYLOAD`] bytes.
    TooLarge,
    /// The service has no such method.
    UnknownMethod,
    /// Only the TV app may change Prysm.
    Forbidden,
    /// The service is being removed.
    Removing,
    /// The payload is not this method's request.
    InvalidRequest,
    /// The other package's method: `setTarget` on slim, `setEnabled` on full.
    WrongPackage,
    /// A preference the installed package does not have.
    InvalidPreferences,
    /// The preferences could not be saved.
    SaveFailed,
    /// The bus could not read the call or send the reply.
    Internal,
}

/// A failed call: its kind and the text for people.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Refusal {
    kind: ErrorKind,
    text: String,
}

impl Refusal {
    fn new(kind: ErrorKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
        }
    }
}

/// A successful state reply: [`State`] with LS2's `returnValue`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct StateReply {
    #[serde(flatten)]
    state: State,
    #[cfg_attr(feature = "schema", schema(schema_with = crate::schema::always_true))]
    return_value: bool,
    /// On `getState` and subscription updates: whether the caller is subscribed.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schema(nullable = false))]
    subscribed: Option<bool>,
}

impl StateReply {
    #[must_use]
    pub fn new(state: State, subscribed: Option<bool>) -> Self {
        Self {
            state,
            return_value: true,
            subscribed,
        }
    }
}

/// The `uninstall` reply.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DoneReply {
    #[cfg_attr(feature = "schema", schema(schema_with = crate::schema::always_true))]
    return_value: bool,
}

impl Default for DoneReply {
    fn default() -> Self {
        Self { return_value: true }
    }
}

/// Any failed call. LS2's own replies (for example for a method that is not
/// registered) carry no `errorKind`; an app reads that as an unknown kind.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ErrorReply {
    #[cfg_attr(feature = "schema", schema(schema_with = crate::schema::always_false))]
    return_value: bool,
    error_kind: ErrorKind,
    /// What failed, for people.
    error_text: String,
}

impl ErrorReply {
    #[must_use]
    pub fn new(kind: ErrorKind, text: impl Into<String>) -> Self {
        Self {
            return_value: false,
            error_kind: kind,
            error_text: text.into(),
        }
    }
}

/// A reply's payload: compact JSON, its keys sorted at every level (through
/// a [`Value`]).
pub(crate) fn encode_reply(reply: &impl Serialize) -> String {
    serde_json::to_value(reply)
        .expect("a Luna reply has only string keys")
        .to_string()
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
    /// Waiting for the current worker failed (and was logged).
    wait_failed: bool,
    renew: Renewals,
    video_context: Option<Value>,
    published: Option<String>,
    /// `tv.json` as last read or saved; the default while it is unreadable,
    /// so the next save replaces it.
    file: PreferencesFile,
    /// The intent last given to the store.
    remembered: Option<LightingTarget>,
    uninstall: Uninstall,
}

impl<L: Launcher, S: PreferencesStore, I: Installer> Service<L, S, I> {
    /// Load preferences. Unreadable ones are reported (the state's
    /// `unreadable`), the defaults used, and the file left as it is until
    /// the app saves preferences.
    pub fn new(config: Config, launcher: L, mut store: S, installer: I, now: Instant) -> Self {
        let flavor = config.flavor;
        let loaded = store
            .load()
            .map_err(|error| chain(&error))
            .and_then(|file| {
                let file = file.unwrap_or_default();
                let preferences = file
                    .preferences(flavor)
                    .map_err(|error| format!("invalid saved preferences: {error}"))?;
                Ok((file, preferences))
            });
        let (file, preferences, unreadable) = match loaded {
            Ok((file, preferences)) => (file, preferences, None),
            Err(why) => {
                warn!(error = %why, "using the default preferences");
                (
                    PreferencesFile::default(),
                    Preferences::default(),
                    Some(why),
                )
            }
        };
        // A restart within one boot (an update, a crash) resumes the intent.
        let resumed = store.resume();
        let mut machine = match resumed {
            Some(intent) => Machine::resumed(config.spec.clone(), preferences, intent),
            None => Machine::new(config.spec.clone(), preferences),
        };
        if let Some(why) = unreadable {
            machine = machine.with_unreadable_preferences(why);
        }
        Self {
            config,
            machine,
            launcher,
            store,
            installer,
            worker: None,
            wait_failed: false,
            renew: Renewals {
                power: Some(now + RESUBSCRIBE),
                video: Some(now + RESUBSCRIBE),
            },
            video_context: None,
            published: None,
            file,
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
            Err(Refusal { kind, text }) => Reply {
                payload: encode_reply(&ErrorReply::new(kind, text)),
                subscribe: false,
            },
        }
    }

    fn try_request(
        &mut self,
        bus: &mut impl Bus,
        method: &str,
        sender: &str,
        payload: &str,
        now: Instant,
    ) -> Result<Reply, Refusal> {
        if payload.len() > MAX_PAYLOAD {
            return Err(Refusal::new(
                ErrorKind::TooLarge,
                format!("payload exceeds {MAX_PAYLOAD} bytes"),
            ));
        }
        let Some(method) = Method::parse(method) else {
            return Err(Refusal::new(
                ErrorKind::UnknownMethod,
                format!("unknown method `{method}`"),
            ));
        };
        if method != Method::GetState && sender != self.config.app_id {
            // Checked before the payload is even parsed.
            return Err(Refusal::new(
                ErrorKind::Forbidden,
                format!("only {} may change Prysm", self.config.app_id),
            ));
        }
        if self.uninstall == Uninstall::Requested {
            return Err(Refusal::new(
                ErrorKind::Removing,
                "the service is being removed",
            ));
        }
        let name = method.name();
        let invalid = |why: &dyn std::fmt::Display| {
            Refusal::new(
                ErrorKind::InvalidRequest,
                format!("invalid {name} request: {why}"),
            )
        };
        let parse_error = |error: serde_json::Error| invalid(&error);
        // Only objects: serde would also accept a struct as an array.
        let payload = match serde_json::from_str::<Value>(payload).map_err(parse_error)? {
            Value::Object(object) => Value::Object(object),
            _ => return Err(invalid(&"expected an object")),
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
                let flavor = self.config.flavor;
                preferences
                    .validate(flavor)
                    .map_err(|why| Refusal::new(ErrorKind::InvalidPreferences, why))?;
                // Only the package's own fields change; the rest of the
                // file is written back as read.
                let mut file = self.file.clone();
                file.replace(&preferences, flavor);
                self.store
                    .save(&file)
                    .map_err(|error| Refusal::new(ErrorKind::SaveFailed, chain(&error)))?;
                self.file = file;
                self.input(bus, Input::Preferences(preferences), now);
                Ok(self.state_reply())
            }
            Method::SetTarget => {
                self.only_for(Flavor::Full, name)?;
                let request: SetTargetRequest =
                    serde_json::from_value(payload).map_err(parse_error)?;
                self.input(bus, Input::Target(request.target), now);
                Ok(self.state_reply())
            }
            Method::SetEnabled => {
                self.only_for(Flavor::Slim, name)?;
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
                    payload: encode_reply(&DoneReply::default()),
                    subscribe: false,
                })
            }
        }
    }

    /// Refuse `method`, the other package's, before its request is decoded.
    fn only_for(&self, flavor: Flavor, method: &str) -> Result<(), Refusal> {
        if self.config.flavor == flavor {
            return Ok(());
        }
        Err(Refusal::new(
            ErrorKind::WrongPackage,
            format!("{method} is only for the {} package", flavor.as_str()),
        ))
    }

    /// A `tvpower` subscription reply.
    pub fn power(&mut self, bus: &mut impl Bus, payload: &str, now: Instant) {
        let reading = serde_json::from_str(payload)
            .ok()
            .and_then(|value: Value| observe::power(&value));
        let slot = self.renew.of(Watch::Power);
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
        let slot = self.renew.of(Watch::Video);
        if reading.live {
            *slot = None;
        } else {
            // The reading is Unknown until it is live: capture still tries,
            // bounded by the adapter.
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

    /// A line for the state's tail.
    pub fn log(&mut self, bus: &mut impl Bus, line: String, now: Instant) {
        self.input(bus, Input::Log(line), now);
    }

    /// Stop the worker and then the service.
    pub fn shutdown(&mut self, bus: &mut impl Bus, now: Instant) {
        self.input(bus, Input::Shutdown, now);
    }

    /// Called at every event (a request, a reply, the worker said something
    /// or exited, a signal) and at [`Self::next_due`]: reads and reaps the
    /// worker, runs the machine's due times and renews subscriptions.
    /// Returns `false` once the service should exit.
    pub fn tick(&mut self, bus: &mut impl Bus, now: Instant) -> bool {
        self.poll_worker(bus, now);
        if self.held().is_none() {
            self.input(bus, Input::Tick, now);
        }
        for watch in [Watch::Power, Watch::Video] {
            let slot = self.renew.of(watch);
            if slot.is_some_and(|at| now >= at) {
                *slot = Some(now + RESUBSCRIBE);
                debug!(?watch, "renewing a subscription that is not live");
                bus.resubscribe(watch);
            }
        }
        if !self.machine.finished() {
            return true;
        }
        if self.uninstall == Uninstall::Requested
            && let Err(error) = self.installer.uninstall()
        {
            error!(error = %chain(&error), "uninstall failed");
        }
        false
    }

    /// When [`Self::tick`] next has something to do without an event: the
    /// machine's next due time, or the end of the wait for an exited
    /// worker's last lines instead, and a subscription renewal.
    #[must_use]
    pub fn next_due(&self) -> Option<Instant> {
        [
            self.held().or_else(|| self.machine.next_due()),
            self.renew.power,
            self.renew.video,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Until when a reaped worker's exit is held back for its last lines
    /// ([`Worker::due`]). Meanwhile the machine's due times wait: they stop
    /// or let go of a live process, and this one's exit comes by then.
    fn held(&self) -> Option<Instant> {
        self.worker.as_ref().and_then(Worker::due)
    }

    fn poll_worker(&mut self, bus: &mut impl Bus, now: Instant) {
        let Some(worker) = &mut self.worker else {
            return;
        };
        let mut outputs = worker.drain();
        let exit = match worker.try_exit() {
            Ok(exit) => exit,
            Err(error) => {
                // Logged once per worker; the machine lets it go after its kill.
                if !self.wait_failed {
                    self.wait_failed = true;
                    warn!(%error, "cannot wait for the worker");
                }
                None
            }
        };
        if exit.is_some() {
            // Its last lines, all read by now (see `Worker::try_exit`).
            outputs.extend(worker.drain());
            self.worker = None;
        }
        for output in outputs {
            let input = match output {
                Output::Event(event) => Input::Worker(event),
                Output::Log(line) => Input::Log(line),
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
                warn!(error = %chain(&error), "cannot remember the intent");
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
                        Ok(worker) => {
                            self.worker = Some(worker);
                            self.wait_failed = false;
                        }
                        Err(error) => failed_spawn = Some(error.to_string()),
                    }
                }
                Effect::Send(command) => {
                    if let Some(worker) = &mut self.worker {
                        // A worker that stopped reading is exiting; its exit follows.
                        let _ = worker.send(&command);
                    }
                }
                // A signal that cannot be sent (the worker already exited)
                // is logged; its exit follows either way.
                Effect::Terminate => {
                    if let Some(worker) = &mut self.worker
                        && let Err(error) = worker.terminate()
                    {
                        warn!(%error, "cannot stop the worker");
                    }
                }
                Effect::Kill => {
                    if let Some(worker) = &mut self.worker
                        && let Err(error) = worker.kill()
                    {
                        warn!(%error, "cannot kill the worker");
                    }
                }
                // Dropping it reaps it on a thread, if it ever can be.
                Effect::Abandon => {
                    if self.worker.take().is_some() {
                        warn!("letting go of a worker not reaped after its kill");
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
        encode_reply(&StateReply::new(self.machine.state(), subscribed))
    }
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

/// `setTarget` request (full package).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct SetTargetRequest {
    /// What the lights do: off, screen sync, or their own effects.
    #[cfg_attr(feature = "schema", schema(value_type = crate::schema::LightingTargetSchema))]
    pub target: LightingTarget,
}

/// `setEnabled` request (slim package).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct SetEnabledRequest {
    /// Frames should flow to the receiver.
    pub enabled: bool,
}

/// `uninstall` request: an empty object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct UninstallRequest {}

#[cfg(test)]
pub(crate) mod tests;
