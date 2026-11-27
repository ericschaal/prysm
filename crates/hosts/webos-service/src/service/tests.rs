//! The driver against a fake bus, fake workers, memory preferences and a
//! fake installer: requests, subscriptions, worker I/O and uninstall.
use super::*;
use crate::identity::APP_ID;
use crate::install::InstallError;
use crate::launcher::Output;
use crate::machine::Exit;
use crate::preferences::{FileStore, MemoryStore, StoreError};
use prysm_webos_link::{Command, Event, FullStatus, Video, WorkerStatus};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::rc::Rc;

#[derive(Default)]
pub(crate) struct World {
    pub log: Vec<String>,
    pub outputs: VecDeque<Output>,
    pub exit: Option<Exit>,
    pub refuse_spawn: bool,
    /// `waitpid` fails for every worker.
    pub wait_fails: bool,
    /// How many worker handles were let go.
    pub dropped: u32,
    /// Reaped, its exit held back for its last lines until then.
    pub held: Option<Instant>,
    /// A worker let go no longer exists, until the launcher is next asked.
    pub let_go_reaped: bool,
}

#[derive(Clone, Default)]
pub(crate) struct FakeLauncher(pub Rc<RefCell<World>>);

pub(crate) struct FakeWorker(Rc<RefCell<World>>);

impl Launcher for FakeLauncher {
    type Worker = FakeWorker;
    fn spawn(&mut self, program: &Path, args: &[String]) -> io::Result<FakeWorker> {
        let mut world = self.0.borrow_mut();
        if world.refuse_spawn {
            return Err(io::Error::other("no such file"));
        }
        world
            .log
            .push(format!("spawn {} {}", program.display(), args.join(" ")));
        Ok(FakeWorker(self.0.clone()))
    }
    fn let_go_reaped(&mut self) -> bool {
        std::mem::take(&mut self.0.borrow_mut().let_go_reaped)
    }
}

impl Worker for FakeWorker {
    fn send(&mut self, command: &Command) -> io::Result<()> {
        self.0
            .borrow_mut()
            .log
            .push(format!("send {}", serde_json::to_string(command).unwrap()));
        Ok(())
    }
    fn terminate(&mut self) -> io::Result<()> {
        self.0.borrow_mut().log.push("terminate".into());
        Ok(())
    }
    fn kill(&mut self) -> io::Result<()> {
        self.0.borrow_mut().log.push("kill".into());
        Ok(())
    }
    fn drain(&mut self) -> Vec<Output> {
        self.0.borrow_mut().outputs.drain(..).collect()
    }
    fn try_exit(&mut self) -> io::Result<Option<Exit>> {
        let mut world = self.0.borrow_mut();
        if world.wait_fails {
            return Err(io::Error::other("no child"));
        }
        Ok(world.exit.take())
    }
    fn due(&self) -> Option<Instant> {
        self.0.borrow().held
    }
}

impl Drop for FakeWorker {
    fn drop(&mut self) {
        self.0.borrow_mut().dropped += 1;
    }
}

#[derive(Clone, Default)]
pub(crate) struct FakeInstaller(pub Rc<RefCell<Vec<String>>>);

impl Installer for FakeInstaller {
    fn install(&mut self) -> Result<(), InstallError> {
        self.0.borrow_mut().push("install".into());
        Ok(())
    }
    fn activate(&mut self, service: &str) -> Result<(), InstallError> {
        self.0.borrow_mut().push(format!("activate {service}"));
        Ok(())
    }
    fn uninstall(&mut self) -> Result<(), InstallError> {
        self.0.borrow_mut().push("uninstall".into());
        Ok(())
    }
}

#[derive(Default)]
struct FakeBus {
    published: Vec<Value>,
    resubscribed: Vec<Watch>,
}

impl Bus for FakeBus {
    fn publish(&mut self, payload: &str) {
        self.published.push(serde_json::from_str(payload).unwrap());
    }
    fn resubscribe(&mut self, watch: Watch) {
        self.resubscribed.push(watch);
    }
}

pub(crate) fn config() -> Config {
    Config {
        flavor: Flavor::Full,
        app_id: APP_ID.into(),
        program: "/svc/prysm-webos-worker".into(),
        spec: Spec::Full {
            settings: "/state/settings.json".into(),
            listen: "0.0.0.0:8787".parse().unwrap(),
        },
    }
}

fn slim_config() -> Config {
    Config {
        flavor: Flavor::Slim,
        app_id: APP_ID.into(),
        program: "/svc/prysm-webos-worker".into(),
        spec: Spec::Slim,
    }
}

struct Harness {
    service: Service<FakeLauncher, MemoryStore, FakeInstaller>,
    world: Rc<RefCell<World>>,
    installer: Rc<RefCell<Vec<String>>>,
    bus: FakeBus,
    t0: Instant,
}

impl Harness {
    /// `saved`: the content of `tv.json`, if any.
    fn new(config: Config, saved: Option<Value>) -> Self {
        let launcher = FakeLauncher::default();
        let installer = FakeInstaller::default();
        let t0 = Instant::now();
        let saved = saved.map(|json| serde_json::from_value(json).unwrap());
        Self {
            world: launcher.0.clone(),
            installer: installer.0.clone(),
            service: Service::new(config, launcher, MemoryStore(saved, None), installer, t0),
            bus: FakeBus::default(),
            t0,
        }
    }

    /// `tv.json` as last saved.
    fn saved(&self) -> Value {
        serde_json::to_value(self.service.store.0.as_ref().unwrap()).unwrap()
    }

    fn at(&self, ms: u64) -> Instant {
        self.t0 + Duration::from_millis(ms)
    }

    fn call(&mut self, method: &str, sender: &str, payload: &str) -> Value {
        let now = self.at(0);
        let reply = self
            .service
            .request(&mut self.bus, method, sender, payload, now);
        serde_json::from_str(&reply.payload).unwrap()
    }

    fn app(&mut self, method: &str, payload: &str) -> Value {
        let app = self.service.config.app_id.clone();
        self.call(method, &app, payload)
    }

    fn power(&mut self, state: &str, ms: u64) {
        let now = self.at(ms);
        let payload = format!(r#"{{"returnValue":true,"state":"{state}"}}"#);
        self.service.power(&mut self.bus, &payload, now);
    }

    fn tick(&mut self, ms: u64) -> bool {
        let now = self.at(ms);
        self.service.tick(&mut self.bus, now)
    }

    fn log(&self) -> Vec<String> {
        std::mem::take(&mut self.world.borrow_mut().log)
    }
}

const ACTIVE_VIDEO: &str = r#"{"returnValue":true,"subscribed":true,"video":[{"sink":"MAIN","connected":true,
    "width":1920,"height":1080,"frameRate":60,"connectedSource":"HDMI","connectedSourcePort":1,"context":"a"}]}"#;

#[test]
fn get_state_is_open_and_subscribable() {
    let mut h = Harness::new(config(), None);
    let now = h.at(0);
    let reply = h.service.request(
        &mut h.bus,
        "getState",
        "com.other.app",
        r#"{"subscribe":true}"#,
        now,
    );
    assert!(reply.subscribe);
    let state: Value = serde_json::from_str(&reply.payload).unwrap();
    assert_eq!(state["returnValue"], true);
    assert_eq!(state["subscribed"], true);
    assert_eq!(state["flavor"], "full");
    assert_eq!(state["power"], "unknown");
    assert_eq!(state["worker"], serde_json::json!({ "phase": "stopped" }));
    assert_eq!(h.call("getState", "x", "{}")["subscribed"], false);
    for bad in [r#"{"subscribe":1}"#, r#"{"other":true}"#, "[]", "nope"] {
        assert_eq!(h.call("getState", "x", bad)["returnValue"], false, "{bad}");
    }
}

#[test]
fn subscriptions_publish_only_changes_in_order() {
    let mut h = Harness::new(config(), None);
    for enabled in [true, true, false, false, true] {
        h.app("setPreferences", &format!(r#"{{"autostart":{enabled}}}"#));
    }
    let changes: Vec<_> = h
        .bus
        .published
        .iter()
        .map(|state| state["preferences"]["autostart"].as_bool().unwrap())
        .collect();
    assert_eq!(changes, [true, false, true]);
}

#[test]
fn only_the_app_may_change_anything() {
    let mut h = Harness::new(config(), None);
    for (method, payload) in [
        ("setTarget", r#"{"target":"screen_sync"}"#),
        ("setEnabled", r#"{"enabled":true}"#),
        ("setPreferences", r#"{"autostart":true}"#),
        ("uninstall", "{}"),
    ] {
        let reply = h.call(method, "com.evil.app", payload);
        assert_eq!(reply["returnValue"], false);
        assert_eq!(reply["errorKind"], "forbidden");
        assert!(
            reply["errorText"]
                .as_str()
                .unwrap()
                .contains(&format!("only {APP_ID}"))
        );
    }
    let unknown = h.call("reboot", APP_ID, "{}");
    assert_eq!(unknown["returnValue"], false);
    assert_eq!(unknown["errorKind"], "unknown_method");
    let oversized = format!(r#"{{"target":"off","x":"{}"}}"#, "y".repeat(MAX_PAYLOAD));
    let oversized = h.app("setTarget", &oversized);
    assert_eq!(oversized["errorKind"], "too_large");
    assert!(oversized["errorText"].as_str().unwrap().contains("exceeds"));
    assert!(h.installer.borrow().is_empty());
    assert!(h.service.machine().preferences() == &Preferences::default());
}

#[test]
fn preferences_are_validated_persisted_and_applied() {
    let mut h = Harness::new(slim_config(), None);
    let bad = h.app(
        "setPreferences",
        r#"{"prysm_address":"http://0.0.0.0:8787"}"#,
    );
    assert_eq!(bad["errorKind"], "invalid_request");
    assert_eq!(
        bad["errorText"],
        "invalid setPreferences request: Prysm address `http://0.0.0.0:8787`: needs Prysm's address, not 0.0.0.0"
    );
    let named = h.app(
        "setPreferences",
        r#"{"prysm_address":"https://prysm.example.com"}"#,
    );
    assert_eq!(
        named["preferences"]["prysm_address"],
        "https://prysm.example.com"
    );
    assert_eq!(h.saved()["prysm_address"], "https://prysm.example.com");
    let unknown = h.app("setPreferences", r#"{"mode":"full"}"#);
    assert_eq!(unknown["errorKind"], "invalid_request");
    for array in ["[]", "[true]"] {
        let refused = h.app("setPreferences", array);
        assert_eq!(refused["errorKind"], "invalid_request", "{array}");
    }
    assert_eq!(h.saved()["prysm_address"], "https://prysm.example.com");
    let reply = h.app(
        "setPreferences",
        r#"{"autostart":true,"prysm_address":"http://192.168.1.20:8787"}"#,
    );
    assert_eq!(reply["returnValue"], true);
    assert_eq!(
        reply["preferences"]["prysm_address"],
        "http://192.168.1.20:8787"
    );
    let mut full = Harness::new(config(), None);
    let refused = full.app(
        "setPreferences",
        r#"{"prysm_address":"http://10.0.0.1:8787"}"#,
    );
    assert_eq!(refused["returnValue"], false);
    assert_eq!(refused["errorKind"], "invalid_preferences");
}

/// Both packages install as the same app and share `tv.json`: full keeps
/// slim's address and fields it does not know, and saves them back.
#[test]
fn full_keeps_the_fields_it_does_not_own() {
    let saved = serde_json::json!({
        "autostart": true,
        "prysm_address": "http://10.0.0.1:8787",
        "newer": [1, 2],
    });
    let mut h = Harness::new(config(), Some(saved));
    assert_eq!(
        h.service.machine().preferences(),
        &Preferences {
            autostart: true,
            prysm_address: None
        }
    );
    let state = h.call("getState", "x", "{}");
    assert_eq!(
        state["preferences"],
        serde_json::json!({ "autostart": true })
    );
    assert_eq!(state.get("failure"), None);
    h.app("setPreferences", r#"{"autostart":false}"#);
    assert_eq!(
        h.saved(),
        serde_json::json!({
            "autostart": false,
            "prysm_address": "http://10.0.0.1:8787",
            "newer": [1, 2],
        })
    );
    let mut slim = Harness::new(slim_config(), Some(h.saved()));
    assert_eq!(
        slim.call("getState", "x", "{}")["preferences"],
        serde_json::json!({ "autostart": false, "prysm_address": "http://10.0.0.1:8787" })
    );
}

/// A store whose file cannot be read and cannot be replaced.
#[derive(Default)]
struct Broken {
    saves: u32,
}

impl PreferencesStore for Broken {
    fn load(&mut self) -> Result<Option<PreferencesFile>, StoreError> {
        Err(StoreError::Read {
            path: "/state/tv.json".into(),
            source: io::Error::other("unreadable"),
        })
    }
    fn save(&mut self, _: &PreferencesFile) -> Result<(), StoreError> {
        self.saves += 1;
        Err(StoreError::Save {
            path: "/state/tv.json".into(),
            source: io::Error::other("read-only"),
        })
    }
    fn resume(&mut self) -> Option<LightingTarget> {
        None
    }
    fn remember(&mut self, _: LightingTarget) -> Result<(), StoreError> {
        Ok(())
    }
}

/// A save that fails is `save_failed`; nothing it would have saved is
/// applied, so the unreadable file is still the failure.
#[test]
fn a_failed_save_changes_nothing() {
    let mut service = Service::new(
        config(),
        FakeLauncher::default(),
        Broken::default(),
        FakeInstaller::default(),
        Instant::now(),
    );
    let mut bus = FakeBus::default();
    let mut call = |method: &str, payload: &str| -> Value {
        let reply = service.request(&mut bus, method, APP_ID, payload, Instant::now());
        serde_json::from_str(&reply.payload).unwrap()
    };
    let refused = call("setPreferences", r#"{"autostart":true}"#);
    assert_eq!(refused["returnValue"], false);
    assert_eq!(refused["errorKind"], "save_failed");
    assert!(
        refused["errorText"].as_str().unwrap().contains("read-only"),
        "{refused}"
    );
    let state = call("getState", "{}");
    assert!(state["unreadable"].is_string(), "{state}");
    assert_eq!(
        state["preferences"],
        serde_json::json!({ "autostart": false })
    );
    assert_eq!(service.store.saves, 1);
    assert_eq!(service.file, PreferencesFile::default());
    assert_eq!(service.machine().preferences(), &Preferences::default());
}

#[test]
fn slim_resumes_manual_enablement_only_while_the_boot_local_file_exists() {
    use LightingTarget::{Off, ScreenSync};
    for (autostart, boot_intent, manual_intent) in
        [(false, Off, ScreenSync), (true, ScreenSync, Off)]
    {
        let data = tempfile::tempdir().unwrap();
        let run = tempfile::tempdir().unwrap();
        let prefs = data.path().join("tv.json");
        let resume = run.path().join("resume.json");
        std::fs::write(&prefs, format!(r#"{{"autostart":{autostart}}}"#)).unwrap();
        let legacy = serde_json::json!({"boot": "reused-id", "intent": manual_intent});
        std::fs::write(data.path().join("resume.json"), legacy.to_string()).unwrap();
        let start = || {
            Service::new(
                slim_config(),
                FakeLauncher::default(),
                FileStore::new(&prefs, &resume),
                FakeInstaller::default(),
                Instant::now(),
            )
        };
        let mut service = start();
        assert_eq!(service.machine().intent(), boot_intent);
        let reply = service.request(
            &mut FakeBus::default(),
            "setEnabled",
            APP_ID,
            &format!(r#"{{"enabled":{}}}"#, !autostart),
            Instant::now(),
        );
        assert_eq!(
            serde_json::from_str::<Value>(&reply.payload).unwrap()["returnValue"],
            true
        );
        drop(service);
        assert_eq!(start().machine().intent(), manual_intent);
        std::fs::remove_file(&resume).unwrap();
        assert_eq!(start().machine().intent(), boot_intent);
    }
}

/// No `tv.json` yet: the defaults, and no failure.
#[test]
fn a_missing_preferences_file_is_no_failure() {
    let dir = tempfile::tempdir().unwrap();
    let service = Service::new(
        slim_config(),
        FakeLauncher::default(),
        FileStore::new(dir.path().join("tv.json"), dir.path().join("resume.json")),
        FakeInstaller::default(),
        Instant::now(),
    );
    let state = service.machine().state();
    assert_eq!(state.unreadable, None);
    assert_eq!(state.preferences, Preferences::default());
}

/// An unreadable `tv.json` is reported as `unreadable`, the defaults are
/// used, and the file stays as it is until preferences are
/// saved.
#[test]
fn unreadable_preferences_are_reported_and_kept_until_saved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tv.json");
    for (config, content) in [
        (config(), r#"{"autostart":"yes"}"#),
        (config(), "not json"),
        (slim_config(), r#"{"prysm_address":"http://x/path"}"#),
    ] {
        std::fs::write(&path, content).unwrap();
        let mut service = Service::new(
            config,
            FakeLauncher::default(),
            FileStore::new(&path, dir.path().join("resume.json")),
            FakeInstaller::default(),
            Instant::now(),
        );
        let mut bus = FakeBus::default();
        let mut call = |method: &str, payload: &str| -> Value {
            let reply = service.request(&mut bus, method, APP_ID, payload, Instant::now());
            serde_json::from_str(&reply.payload).unwrap()
        };
        let state = call("getState", r#"{"subscribe":true}"#);
        assert!(
            state["unreadable"].as_str().unwrap().contains("invalid"),
            "{state}"
        );
        assert_eq!(
            state["preferences"],
            serde_json::json!({ "autostart": false })
        );
        call("getState", "{}");
        service.power(
            &mut bus,
            r#"{"returnValue":true,"state":"Active"}"#,
            Instant::now(),
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        let mut call = |method: &str, payload: &str| -> Value {
            let reply = service.request(&mut bus, method, APP_ID, payload, Instant::now());
            serde_json::from_str(&reply.payload).unwrap()
        };
        let saved = call("setPreferences", r#"{"autostart":true}"#);
        assert_eq!(saved.get("failure"), None, "{saved}");
        assert_eq!(
            serde_json::from_str::<Value>(&std::fs::read_to_string(&path).unwrap()).unwrap(),
            serde_json::json!({ "autostart": true })
        );
        let published = bus.published.last().unwrap();
        assert_eq!(published.get("failure"), None, "{published}");
    }
}

#[test]
fn power_and_video_drive_the_worker_and_its_commands() {
    let mut h = Harness::new(config(), None);
    h.power("Active", 0);
    assert_eq!(
        h.log(),
        [
            "spawn /svc/prysm-webos-worker --settings /state/settings.json --listen 0.0.0.0:8787",
            r#"send {"type":"video","video":"unknown"}"#,
        ]
    );
    let now = h.at(1);
    h.service.video(&mut h.bus, ACTIVE_VIDEO, now);
    assert_eq!(
        h.log(),
        [
            r#"send {"type":"video","video":"available"}"#,
            r#"send {"type":"observe_video","available":true}"#,
        ]
    );
    let changed = ACTIVE_VIDEO.replace(r#""connectedSourcePort":1"#, r#""connectedSourcePort":2"#);
    let now = h.at(2);
    h.service.video(&mut h.bus, &changed, now);
    assert_eq!(h.log(), [r#"send {"type":"reset_video"}"#]);
    assert_eq!(
        h.app("setTarget", r#"{"target":"effects"}"#)["target"],
        "effects"
    );
    assert_eq!(
        h.log(),
        [r#"send {"type":"target","target":"effects","sequence":1}"#]
    );
    let published = h.bus.published.last().unwrap();
    assert_eq!(published["subscribed"], true);
    assert_eq!(published["power"], "active");
    assert_eq!(published["video"], "available");
}

#[test]
fn worker_output_is_drained_on_tick_and_exits_are_reaped() {
    let mut h = Harness::new(config(), None);
    h.power("Active", 0);
    h.log();
    h.world.borrow_mut().outputs.extend([
        Output::Log("prysm-webos: portal http://10.0.0.2:8787/".into()),
        Output::Event(Event::Status(WorkerStatus::Full(FullStatus {
            capturing: false,
            fps: None,
            target: LightingTarget::Off,
            lights: None,
            applied: 0,
            portal: Some("http://10.0.0.2:8787/".into()),
            failure: None,
        }))),
    ]);
    assert!(h.tick(100));
    let published = h.bus.published.last().unwrap().clone();
    assert_eq!(published["worker"]["portal"], "http://10.0.0.2:8787/");
    assert_eq!(
        published["log"][0],
        "prysm-webos: portal http://10.0.0.2:8787/"
    );

    h.world
        .borrow_mut()
        .outputs
        .push_back(Output::Broken("malformed".into()));
    assert!(h.tick(200));
    assert_eq!(h.log(), ["terminate"]);
    h.world.borrow_mut().exit = Some(Exit::Code(1));
    assert!(h.tick(300));
    let published = h.bus.published.last().unwrap();
    assert_eq!(published["worker"]["phase"], "asking");
    assert_eq!(
        published["worker"]["problem"]["asked"],
        serde_json::json!({ "step": "ask", "fix": "try_again" })
    );
    assert!(
        published["worker"]["problem"]["message"]
            .as_str()
            .unwrap()
            .contains("malformed")
    );
    assert!(h.tick(60_000));
    assert!(h.log().is_empty(), "the person is asked: nothing respawns");
}

/// A worker that dies without a word (its event stream ends cleanly) is
/// restarted at once, with nothing shown; dying again before a frame asks.
#[test]
fn a_stable_worker_is_restarted_and_an_initializing_replacement_is_not() {
    let mut h = Harness::new(config(), None);
    h.power("Active", 0);
    h.log();
    h.world
        .borrow_mut()
        .outputs
        .push_back(Output::Event(Event::Initialized {}));
    h.world.borrow_mut().exit = Some(Exit::Signal(9));
    assert!(h.tick(100));
    let log = h.log();
    assert!(log[0].starts_with("spawn "), "{log:?}");
    let published = h.bus.published.last().unwrap();
    assert_eq!(published["worker"]["phase"], "running");
    assert_eq!(published["worker"].get("problem"), None);
    h.world.borrow_mut().exit = Some(Exit::Signal(9));
    assert!(h.tick(200));
    assert!(h.log().is_empty(), "no second restart");
    let published = h.bus.published.last().unwrap();
    assert_eq!(published["worker"]["phase"], "asking");
    assert!(
        published["worker"]["problem"]["message"]
            .as_str()
            .unwrap()
            .starts_with("worker killed by signal 9")
    );
}

#[test]
fn a_spawn_failure_asks_without_a_worker() {
    let mut h = Harness::new(config(), None);
    h.world.borrow_mut().refuse_spawn = true;
    h.power("Active", 0);
    let published = h.bus.published.last().unwrap();
    assert_eq!(published["worker"]["phase"], "asking");
    assert!(
        published["worker"]["problem"]["message"]
            .as_str()
            .unwrap()
            .contains("no such file")
    );
    assert!(
        h.log().is_empty(),
        "no command reaches a worker that never started"
    );
}

#[test]
fn silent_subscriptions_are_renewed() {
    let mut h = Harness::new(config(), None);
    assert!(h.tick(4_999));
    assert!(h.bus.resubscribed.is_empty());
    assert!(h.tick(5_000));
    assert_eq!(h.bus.resubscribed, [Watch::Power, Watch::Video]);
    // A good power reply settles; an error reply fails closed and renews later.
    h.power("Active", 5_100);
    h.log();
    let now = h.at(5_200);
    h.service.power(&mut h.bus, r#"{"returnValue":false}"#, now);
    assert_eq!(
        h.log(),
        [
            r#"send {"type":"video","video":"unavailable"}"#,
            "terminate"
        ],
        "power unknown is not awake: release the tap, then stop"
    );
    assert!(h.tick(10_000));
    assert_eq!(
        h.bus.resubscribed,
        [Watch::Power, Watch::Video, Watch::Video]
    );
    assert!(h.tick(10_200));
    assert_eq!(h.bus.resubscribed.last(), Some(&Watch::Power));
}

fn video_renewals(h: &Harness) -> usize {
    let renewals = h.bus.resubscribed.iter();
    renewals.filter(|watch| **watch == Watch::Video).count()
}

#[test]
fn a_live_video_subscription_reading_unknown_is_not_renewed() {
    let mut h = Harness::new(config(), None);
    let incomplete = ACTIVE_VIDEO.replace(r#""frameRate":60"#, r#""frameRate":null"#);
    let now = h.at(100);
    h.service.video(&mut h.bus, &incomplete, now);
    assert!(h.tick(5_100));
    assert!(h.tick(60_000));
    assert_eq!(video_renewals(&h), 0, "its next reply says more");
}

#[test]
fn a_lost_video_subscription_reads_unknown_and_is_renewed_until_live() {
    let mut h = Harness::new(config(), None);
    h.power("Active", 0);
    let now = h.at(100);
    h.service.video(&mut h.bus, ACTIVE_VIDEO, now);
    let lost = ACTIVE_VIDEO.replace(r#""subscribed":true"#, r#""subscribed":false"#);
    let now = h.at(200);
    h.service.video(&mut h.bus, &lost, now);
    assert_eq!(h.bus.published.last().unwrap()["video"], "unknown");
    assert!(h.tick(5_199));
    assert_eq!(video_renewals(&h), 0);
    assert!(h.tick(5_200));
    assert_eq!(video_renewals(&h), 1);
    let now = h.at(5_300);
    h.service.video(&mut h.bus, ACTIVE_VIDEO, now);
    assert!(h.tick(60_000));
    assert_eq!(video_renewals(&h), 1, "a live subscription is left alone");
}

#[test]
fn uninstall_stops_the_worker_then_removes_registration_and_exits() {
    let mut h = Harness::new(config(), None);
    h.power("Active", 0);
    h.log();
    assert_eq!(
        h.app("uninstall", "{}"),
        serde_json::json!({"returnValue": true})
    );
    assert_eq!(h.log(), ["terminate"]);
    let refused = h.app("setTarget", r#"{"target":"screen_sync"}"#);
    assert_eq!(refused["returnValue"], false);
    assert_eq!(refused["errorKind"], "removing");
    assert!(h.tick(100), "still waiting for the worker");
    assert!(h.installer.borrow().is_empty());
    h.world.borrow_mut().exit = Some(Exit::Code(0));
    assert!(!h.tick(200));
    assert_eq!(*h.installer.borrow(), ["uninstall"]);
}

#[test]
fn shutdown_waits_for_the_worker() {
    let mut h = Harness::new(config(), None);
    h.power("Active", 0);
    h.log();
    let now = h.at(1);
    h.service.shutdown(&mut h.bus, now);
    assert_eq!(h.log(), ["terminate"]);
    assert!(h.tick(8_000));
    assert_eq!(h.log(), [] as [String; 0]);
    assert!(h.tick(8_001));
    assert_eq!(h.log(), ["kill"]);
    h.world.borrow_mut().exit = Some(Exit::Signal(9));
    assert!(!h.tick(8_100));
    assert!(
        h.installer.borrow().is_empty(),
        "a plain stop keeps registration"
    );
}

/// A worker that cannot be reaped is let go 8 s after its kill:
/// the shutdown finishes and no handle is left behind.
#[test]
fn a_worker_that_cannot_be_reaped_is_let_go_after_its_kill() {
    let mut h = Harness::new(config(), None);
    h.power("Active", 0);
    h.log();
    h.world.borrow_mut().wait_fails = true;
    let now = h.at(1);
    h.service.shutdown(&mut h.bus, now);
    assert!(h.tick(8_001));
    assert_eq!(h.log(), ["terminate", "kill"]);
    assert!(h.tick(16_000));
    assert_eq!(h.world.borrow().dropped, 0);
    assert!(!h.tick(16_001), "the shutdown finishes");
    assert_eq!(h.world.borrow().dropped, 1);
}

/// A worker that cannot be waited for is let go after its kill and the
/// person asked to restart the TV, until its late reap returns: then a new
/// worker starts.
#[test]
fn a_worker_let_go_is_asked_about_until_its_late_reap() {
    let mut h = Harness::new(config(), None);
    h.power("Active", 0);
    h.log();
    h.world.borrow_mut().wait_fails = true;
    h.world
        .borrow_mut()
        .outputs
        .push_back(Output::Broken("malformed".into()));
    assert!(h.tick(1));
    assert!(h.tick(8_001));
    assert!(h.tick(16_001));
    assert_eq!(h.log(), ["terminate", "kill"]);
    assert_eq!(h.world.borrow().dropped, 1);
    let published = h.bus.published.last().unwrap();
    assert_eq!(
        published["worker"]["problem"]["asked"],
        serde_json::json!({ "step": "ask", "fix": "restart_tv" })
    );
    h.world.borrow_mut().wait_fails = false;
    h.world.borrow_mut().let_go_reaped = true;
    assert!(h.tick(16_002));
    assert!(h.log().is_empty());
    let published = h.bus.published.last().unwrap();
    assert_eq!(published["worker"]["phase"], "asking", "{published}");
    assert_eq!(
        published["worker"]["problem"]["message"],
        "worker link broke: malformed"
    );
}

/// A worker reaped just before it would be let go, its last lines still
/// being read, is not let go: its exit comes when the reading ends.
#[test]
fn a_reaped_worker_is_not_let_go_while_its_last_lines_are_read() {
    let mut h = Harness::new(config(), None);
    h.power("Active", 0);
    h.log();
    let now = h.at(1);
    h.service.shutdown(&mut h.bus, now);
    assert!(h.tick(8_001));
    assert_eq!(h.log(), ["terminate", "kill"]);
    h.world.borrow_mut().held = Some(h.at(16_500));
    assert!(h.tick(16_001));
    assert_eq!(h.world.borrow().dropped, 0, "not let go");
    assert_eq!(h.service.next_due(), Some(h.at(16_500)));
    let mut world = h.world.borrow_mut();
    world.held = None;
    world.exit = Some(Exit::Signal(9));
    drop(world);
    assert!(!h.tick(16_500), "the shutdown finishes");
    assert_eq!(h.world.borrow().dropped, 1);
    let published = h.bus.published.last().unwrap();
    assert_eq!(published["worker"]["phase"], "stopped", "{published}");
}

#[test]
fn a_restart_within_one_boot_resumes_the_last_intent() {
    let mut h = Harness::new(config(), None);
    h.power("Active", 0);
    h.log();
    h.app("setTarget", r#"{"target":"screen_sync"}"#);
    assert_eq!(h.service.store.1, Some(LightingTarget::ScreenSync));
    let store = h.service.store.clone();

    let launcher = FakeLauncher::default();
    let world = launcher.0.clone();
    let t0 = Instant::now();
    let mut service = Service::new(config(), launcher, store, FakeInstaller::default(), t0);
    let mut bus = FakeBus::default();
    service.power(&mut bus, r#"{"returnValue":true,"state":"Active"}"#, t0);
    assert_eq!(
        world.borrow().log[0],
        "spawn /svc/prysm-webos-worker --settings /state/settings.json --listen 0.0.0.0:8787"
    );
    assert_eq!(service.machine().intent(), LightingTarget::ScreenSync);

    service.request(&mut bus, "setTarget", APP_ID, r#"{"target":"off"}"#, t0);
    assert_eq!(service.store.1, Some(LightingTarget::Off));
}

/// Picture settings are not TV preferences in either package: the slim
/// worker follows its receiver's request, the full one the portal.
#[test]
fn neither_package_has_a_capture_preference() {
    for config in [slim_config(), config()] {
        let mut harness = Harness::new(config, None);
        let state = harness.call("getState", "x", "{}");
        assert_eq!(state["preferences"].get("capture"), None, "{state}");
        let refused = harness.app("setPreferences", r#"{"capture":{"width":320}}"#);
        assert!(
            refused["errorText"].as_str().unwrap().contains("capture"),
            "{refused}"
        );
    }
    let mut slim = Harness::new(slim_config(), None);
    let reply = slim.app(
        "setPreferences",
        r#"{"prysm_address":"http://192.168.1.20:8787"}"#,
    );
    assert_eq!(
        reply["preferences"]["prysm_address"],
        "http://192.168.1.20:8787"
    );
}

#[test]
fn packaged_api_permissions_list_every_method() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../packaging/webos/service/supervisor.api.json");
    let api: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let service = format!("{APP_ID}.service");
    let expected: Vec<String> = Method::ALL
        .iter()
        .map(|method| format!("{service}/{}", method.name()))
        .collect();
    assert_eq!(api, serde_json::json!({ "public": expected }));
    for (method, name) in Method::ALL.into_iter().zip(METHODS) {
        assert_eq!(method.name(), name);
        assert_eq!(Method::parse(name), Some(method));
    }
    assert_eq!(Method::parse("GetState"), None);
}

/// Each package has its own lighting method and refuses the other's, without
/// changing anything.
#[test]
fn each_package_refuses_the_other_packages_lighting_method() {
    let mut full = Harness::new(config(), None);
    let refused = full.app("setEnabled", r#"{"enabled":true}"#);
    assert_eq!(refused["returnValue"], false);
    assert_eq!(refused["errorKind"], "wrong_package");
    assert_eq!(
        refused["errorText"],
        "setEnabled is only for the slim package"
    );
    assert_eq!(full.service.machine().intent(), LightingTarget::Off);
    assert_eq!(
        full.app("setTarget", r#"{"target":"processed"}"#)["returnValue"],
        false,
        "a target this version does not know is refused"
    );

    let mut slim = Harness::new(slim_config(), None);
    let refused = slim.app("setTarget", r#"{"target":"screen_sync"}"#);
    assert_eq!(refused["returnValue"], false);
    assert_eq!(refused["errorKind"], "wrong_package");
    assert_eq!(
        refused["errorText"],
        "setTarget is only for the full package"
    );
    assert_eq!(slim.service.machine().intent(), LightingTarget::Off);
    assert_eq!(
        slim.app("setEnabled", r#"{"enabled":true}"#)["enabled"],
        true
    );
}

#[test]
fn replies_keep_their_bytes() {
    use crate::state::{FullWorker, Package, SlimWorker};
    let full = State {
        package: Package::Full {
            target: LightingTarget::Off,
            worker: FullWorker::Stopped,
        },
        preferences: Preferences::default(),
        power: Power::Active,
        video: Video::Available,
        unreadable: Some("x".into()),
        log: vec!["a".into()],
    };
    let slim = State {
        package: Package::Slim {
            enabled: true,
            worker: SlimWorker::Stopped,
        },
        unreadable: None,
        log: Vec::new(),
        ..full.clone()
    };
    for (reply, expected) in [
        (
            encode_reply(&StateReply::new(full, Some(true))),
            r#"{"flavor":"full","log":["a"],"power":"active","preferences":{"autostart":false},"returnValue":true,"subscribed":true,"target":"off","unreadable":"x","video":"available","worker":{"phase":"stopped"}}"#,
        ),
        (
            encode_reply(&StateReply::new(slim, None)),
            r#"{"enabled":true,"flavor":"slim","log":[],"power":"active","preferences":{"autostart":false},"returnValue":true,"video":"available","worker":{"phase":"stopped"}}"#,
        ),
        (
            encode_reply(&ErrorReply::new(
                ErrorKind::WrongPackage,
                "setTarget is only for the full package",
            )),
            r#"{"errorKind":"wrong_package","errorText":"setTarget is only for the full package","returnValue":false}"#,
        ),
        (
            encode_reply(&DoneReply::default()),
            r#"{"returnValue":true}"#,
        ),
    ] {
        assert_eq!(reply, expected);
    }
}

#[test]
fn power_from_asking_dispatches_remembered_after_spawn_without_guessing_a_mode() {
    let mut h = Harness::new(config(), None);
    h.power("Active", 0);
    h.log();
    h.world.borrow_mut().exit = Some(Exit::Code(1));
    assert!(h.tick(1));
    assert!(h.log().is_empty());
    assert_eq!(h.bus.published.last().unwrap()["worker"]["phase"], "asking");
    let reply = h.app("setTarget", r#"{"target":"remembered"}"#);
    assert_eq!(reply["returnValue"], true);
    assert_eq!(
        h.log(),
        [
            "spawn /svc/prysm-webos-worker --settings /state/settings.json --listen 0.0.0.0:8787",
            r#"send {"type":"resume","sequence":1}"#,
            r#"send {"type":"video","video":"unknown"}"#,
        ]
    );
}
