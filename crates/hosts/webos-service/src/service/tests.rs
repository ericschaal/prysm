//! The driver against a fake bus, fake workers, memory preferences and a
//! fake installer: requests, subscriptions, worker I/O and uninstall.
use super::*;
use crate::identity::APP_ID;
use crate::install::InstallError;
use crate::launcher::Output;
use crate::machine::Exit;
use crate::preferences::{MemoryStore, StoreError};
use prysm_webos_link::{Command, Event, FullStatus, WorkerStatus};
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
        None
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
    // A host name is saved as typed; the TV resolves it when it connects.
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
    // Full rejects slim-only fields.
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
    // Back on slim, the address is there.
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
    assert_eq!(state["failure"]["kind"], "preferences", "{state}");
    assert_eq!(
        state["preferences"],
        serde_json::json!({ "autostart": false })
    );
    assert_eq!(service.store.saves, 1);
    assert_eq!(service.file, PreferencesFile::default());
    assert_eq!(service.machine().preferences(), &Preferences::default());
}

/// No `tv.json` yet: the defaults, and no failure.
#[test]
fn a_missing_preferences_file_is_no_failure() {
    let dir = tempfile::tempdir().unwrap();
    let service = Service::new(
        slim_config(),
        FakeLauncher::default(),
        crate::preferences::FileStore::new(dir.path().join("tv.json"))
            .with_boot_id(dir.path().join("no-boot-id")),
        FakeInstaller::default(),
        Instant::now(),
    );
    let state = service.machine().state(Instant::now());
    assert_eq!(state.failure, None);
    assert_eq!(state.preferences, Preferences::default());
}

/// An unreadable `tv.json` is reported as the `preferences` failure, the
/// defaults are used, and the file stays as it is until preferences are
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
            crate::preferences::FileStore::new(&path).with_boot_id(dir.path().join("no-boot-id")),
            FakeInstaller::default(),
            Instant::now(),
        );
        let mut bus = FakeBus::default();
        let mut call = |method: &str, payload: &str| -> Value {
            let reply = service.request(&mut bus, method, APP_ID, payload, Instant::now());
            serde_json::from_str(&reply.payload).unwrap()
        };
        let state = call("getState", r#"{"subscribe":true}"#);
        assert_eq!(state["failure"]["kind"], "preferences", "{content}");
        assert!(
            state["failure"]["message"]
                .as_str()
                .unwrap()
                .contains("invalid"),
            "{state}"
        );
        assert_eq!(
            state["preferences"],
            serde_json::json!({ "autostart": false })
        );
        // Starting and stopping changes nothing on disk.
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
            "spawn /svc/prysm-webos-worker --settings /state/settings.json --listen 0.0.0.0:8787 --target off",
            r#"send {"type":"video","video":"unknown"}"#,
        ]
    );
    let now = h.at(1);
    h.service.video(&mut h.bus, ACTIVE_VIDEO, now);
    assert_eq!(h.log(), [r#"send {"type":"video","video":"available"}"#]);
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
    assert_eq!(
        published["worker"],
        serde_json::json!({ "phase": "retrying", "seconds": 1 })
    );
    assert_eq!(published["failure"]["kind"], "other");
    assert!(
        published["failure"]["message"]
            .as_str()
            .unwrap()
            .contains("malformed")
    );
    assert!(h.tick(1_299));
    assert!(h.log().is_empty());
    assert!(h.tick(1_300));
    assert_eq!(h.log().len(), 2, "respawned after one second");
}

#[test]
fn a_spawn_failure_retries_without_a_worker() {
    let mut h = Harness::new(config(), None);
    h.world.borrow_mut().refuse_spawn = true;
    h.power("Active", 0);
    let published = h.bus.published.last().unwrap();
    assert_eq!(published["worker"]["phase"], "retrying");
    assert!(
        published["failure"]["message"]
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

#[test]
fn a_restart_within_one_boot_resumes_the_last_intent() {
    // Lighting was turned on (autostart off), then the service restarted.
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
        "spawn /svc/prysm-webos-worker --settings /state/settings.json --listen 0.0.0.0:8787 --target screen_sync"
    );
    assert_eq!(service.machine().intent(), LightingTarget::ScreenSync);

    // Turning it off is remembered too.
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
    for method in Method::ALL {
        assert_eq!(Method::parse(method.name()), Some(method));
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
