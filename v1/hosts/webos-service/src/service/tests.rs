//! The driver against a fake bus, fake workers, memory preferences and a
//! fake installer: requests, subscriptions, worker I/O and uninstall.
use super::*;
use crate::launcher::Output;
use crate::machine::Exit;
use crate::preferences::MemoryStore;
use prysm_webos_link::{Command, Event, WorkerStatus};
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
        Ok(self.0.borrow_mut().exit.take())
    }
}

#[derive(Clone, Default)]
pub(crate) struct FakeInstaller(pub Rc<RefCell<Vec<String>>>);

impl Installer for FakeInstaller {
    fn install(&mut self) -> Result<(), String> {
        self.0.borrow_mut().push("install".into());
        Ok(())
    }
    fn activate(&mut self, service: &str) -> Result<(), String> {
        self.0.borrow_mut().push(format!("activate {service}"));
        Ok(())
    }
    fn uninstall(&mut self) -> Result<(), String> {
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
        app_id: "io.prysm.tv".into(),
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
        app_id: "io.prysm.tv".into(),
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
    fn new(config: Config, saved: Option<Preferences>) -> Self {
        let launcher = FakeLauncher::default();
        let installer = FakeInstaller::default();
        let t0 = Instant::now();
        Self {
            world: launcher.0.clone(),
            installer: installer.0.clone(),
            service: Service::new(config, launcher, MemoryStore(saved), installer, t0),
            bus: FakeBus::default(),
            t0,
        }
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
    assert_eq!(state["worker"], "stopped");
    assert_eq!(h.call("getState", "x", "{}")["subscribed"], false);
    for bad in [r#"{"subscribe":1}"#, r#"{"other":true}"#, "[]", "nope"] {
        assert_eq!(h.call("getState", "x", bad)["returnValue"], false, "{bad}");
    }
}

#[test]
fn only_the_app_may_change_anything() {
    let mut h = Harness::new(config(), None);
    for (method, payload) in [
        ("setEnabled", r#"{"enabled":true}"#),
        ("setPreferences", r#"{"autostart":true}"#),
        ("uninstall", "{}"),
    ] {
        let reply = h.call(method, "com.evil.app", payload);
        assert_eq!(reply["returnValue"], false);
        assert!(
            reply["errorText"]
                .as_str()
                .unwrap()
                .contains("only io.prysm.tv")
        );
    }
    assert_eq!(h.call("reboot", "io.prysm.tv", "{}")["returnValue"], false);
    let oversized = format!(r#"{{"enabled":true,"x":"{}"}}"#, "y".repeat(MAX_PAYLOAD));
    assert!(
        h.app("setEnabled", &oversized)["errorText"]
            .as_str()
            .unwrap()
            .contains("exceeds")
    );
    assert!(h.installer.borrow().is_empty());
    assert!(h.service.machine().preferences() == &Preferences::default());
}

#[test]
fn preferences_are_validated_persisted_and_applied() {
    let mut h = Harness::new(slim_config(), None);
    let bad = h.app("setPreferences", r#"{"receiver":"0.0.0.0:7380"}"#);
    assert!(bad["errorText"].as_str().unwrap().starts_with("receiver"));
    assert_eq!(
        h.app("setPreferences", r#"{"mode":"full"}"#)["returnValue"],
        false
    );
    let reply = h.app(
        "setPreferences",
        r#"{"autostart":true,"instance":"http://192.168.1.20:8787","receiver":"192.168.1.20:7380"}"#,
    );
    assert_eq!(reply["returnValue"], true);
    assert_eq!(reply["preferences"]["receiver"], "192.168.1.20:7380");
    assert_eq!(
        h.service.store.0.as_ref().unwrap().instance.as_deref(),
        Some("http://192.168.1.20:8787")
    );
    // Full rejects slim-only fields.
    let mut full = Harness::new(config(), None);
    assert_eq!(
        full.app("setPreferences", r#"{"receiver":"10.0.0.1:7380"}"#)["returnValue"],
        false
    );
}

#[test]
fn invalid_saved_preferences_fall_back_to_defaults() {
    let saved = Preferences {
        receiver: Some("10.0.0.1:7380".parse().unwrap()),
        ..Preferences::default()
    };
    let h = Harness::new(config(), Some(saved));
    assert_eq!(h.service.machine().preferences(), &Preferences::default());
}

#[test]
fn power_and_video_drive_the_worker_and_its_commands() {
    let mut h = Harness::new(config(), None);
    h.power("Active", 0);
    assert_eq!(
        h.log(),
        [
            "spawn /svc/prysm-webos-worker --settings /state/settings.json --listen 0.0.0.0:8787 --lighting off",
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
    assert_eq!(h.app("setEnabled", r#"{"enabled":true}"#)["enabled"], true);
    assert_eq!(
        h.log(),
        [r#"send {"type":"lighting","target":"processed"}"#]
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
        Output::Event(Event::Status(WorkerStatus {
            portal: Some("http://10.0.0.2:8787/".into()),
            target: Some(prysm_webos_link::LightingTarget::Off),
            ..WorkerStatus::default()
        })),
    ]);
    assert!(h.tick(100));
    let published = h.bus.published.last().unwrap().clone();
    assert_eq!(published["portal"], "http://10.0.0.2:8787/");
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
    assert_eq!(published["worker"], "retrying");
    assert!(published["failure"].as_str().unwrap().contains("malformed"));
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
    assert_eq!(published["worker"], "retrying");
    assert!(
        published["failure"]
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
    assert_eq!(
        h.app("setEnabled", r#"{"enabled":true}"#)["returnValue"],
        false
    );
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
