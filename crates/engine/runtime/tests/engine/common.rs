//! Set-up shared by the engine tests: settings, the engine with fakes, waits.
use prysm_ports::{
    AdapterSettings, Processing, Registry, Revision, Settings, SettingsStore, Snapshot,
};
use prysm_runtime::fakes::{
    CallLog, FakeOutputSettings, FakeOutputs, FakeSourceSettings, FakeSources, MemoryStore,
};
use prysm_runtime::{Engine, EngineHandle};
use std::time::Duration;

pub const DEADLINE: Duration = Duration::from_secs(2);

/// Longer than any test's own waits, such as the 8 s `behind_within`, so a
/// slow test fails with its own message.
pub const LIMIT: Duration = Duration::from_secs(20);
pub use prysm_testkit::{eventually_async, within_async};

pub fn settings(source: FakeSourceSettings, output: FakeOutputSettings) -> Settings {
    Settings {
        capture: Some(AdapterSettings::new("fake", &source)),
        output: Some(AdapterSettings::new("fake", &output)),
        processing: Processing::default(),
    }
}

pub fn named(source: &str, output: &str) -> Settings {
    settings(
        FakeSourceSettings {
            name: source.into(),
            ..Default::default()
        },
        FakeOutputSettings {
            name: output.into(),
            ..Default::default()
        },
    )
}

pub fn registry(log: &CallLog) -> Registry {
    let mut registry = Registry::default();
    registry.add_source(FakeSources(log.clone()));
    registry.add_output(FakeOutputs(log.clone()));
    registry
}

pub async fn start(
    log: &CallLog,
    settings: Settings,
    deadline: Duration,
) -> (EngineHandle, Engine, MemoryStore) {
    let store = MemoryStore::with(settings);
    let (handle, engine) = Engine::start(registry(log), Box::new(store.clone()), deadline)
        .await
        .unwrap();
    (handle, engine, store)
}

pub async fn snapshot_until(
    handle: &EngineHandle,
    done: impl FnMut(&Snapshot) -> bool,
) -> Snapshot {
    handle.snapshot().wait_for(done).await.unwrap().clone()
}

pub fn revision(handle: &EngineHandle) -> Revision {
    handle.snapshot().borrow().revision
}

pub fn before(log: &CallLog, earlier: &str, later: &str) -> bool {
    match (log.last_position(earlier), log.position(later)) {
        (Some(earlier), Some(later)) => earlier < later,
        _ => false,
    }
}

/// Far longer than [`STUCK_DEADLINE`]: a call that, for the test, never returns.
pub const STUCK: Duration = Duration::from_secs(1);

pub const STUCK_DEADLINE: Duration = Duration::from_millis(100);

/// Saves block for [`STUCK`], like a disk that does not answer.
pub struct SlowStore {
    pub saved: MemoryStore,
    pub log: CallLog,
}

impl SettingsStore for SlowStore {
    fn load(&mut self) -> prysm_ports::Result<Option<Settings>> {
        self.saved.load()
    }

    fn save(&mut self, settings: &Settings) -> prysm_ports::Result<()> {
        self.log.record("store.saving");
        std::thread::sleep(STUCK);
        self.saved.save(settings)
    }
}

/// Shutdown, required to finish well before a stuck call returns.
pub async fn shutdown_before_stuck_call_returns(engine: Engine) -> prysm_ports::Result<()> {
    tokio::time::timeout(STUCK - STUCK_DEADLINE * 2, engine.shutdown())
        .await
        .expect("shutdown is bounded")
}
