//! Tried processing: a control being dragged lights with its value at once,
//! without saving it or changing the revision; a save or a timeout ends it.
//!
//! The fake source fills its frames with gray and smoothing is off, so the
//! brightness shows in the preview's green at once.
use prysm_ports::{AdapterSettings, Error, LightingTarget, Processing, Registry, Settings};
use prysm_runtime::fakes::{
    CallLog, FakeOutputSettings, FakeOutputs, FakeSourceSettings, FakeSources, MemoryStore,
};
use prysm_runtime::{Engine, EngineHandle, TRIAL_TIMEOUT};
use std::future::Future;
use std::time::Duration;

const DEADLINE: Duration = Duration::from_secs(2);

async fn within<T>(test: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(20), test)
        .await
        .expect("test timed out")
}

/// Gray frames, no smoothing, full brightness.
fn settings() -> Settings {
    Settings {
        capture: Some(AdapterSettings::new(
            "fake",
            &FakeSourceSettings {
                name: "a".into(),
                fill: 128,
                ..FakeSourceSettings::default()
            },
        )),
        output: Some(AdapterSettings::new(
            "fake",
            &FakeOutputSettings {
                name: "a".into(),
                ..FakeOutputSettings::default()
            },
        )),
        processing: Processing {
            smoothing_seconds: 0.0,
            brightness_percent: 100,
            ..Processing::default()
        },
    }
}

fn brightness(percent: u8) -> Processing {
    Processing {
        brightness_percent: percent,
        ..settings().processing
    }
}

async fn start(store: &MemoryStore) -> (EngineHandle, Engine) {
    let log = CallLog::new();
    let mut registry = Registry::default();
    registry.add_source(FakeSources(log.clone()));
    registry.add_output(FakeOutputs(log));
    Engine::start(registry, Box::new(store.clone()), DEADLINE)
        .await
        .unwrap()
}

/// Wait up to five seconds (real time) for the preview's green to satisfy `done`.
async fn wait_for_green(handle: &EngineHandle, what: &str, done: impl Fn(f32) -> bool) -> f32 {
    let _viewer = handle.preview();
    let mut seen = None;
    for _ in 0..1000 {
        if let Some(latest) = handle.preview().latest() {
            let green = latest.colors.top.sample_at(0.5).g;
            if done(green) {
                return green;
            }
            seen = Some(green);
        }
        std::thread::sleep(Duration::from_millis(5));
        tokio::task::yield_now().await;
    }
    panic!("preview never became {what}; last green {seen:?}");
}

#[tokio::test]
async fn a_trial_lights_without_saving_and_a_save_ends_it() {
    within(async {
        let store = MemoryStore::with(settings());
        let (handle, engine) = start(&store).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        let full = wait_for_green(&handle, "lit", |g| g > 0.0).await;
        let before = handle.snapshot().borrow().clone();

        handle.try_processing(brightness(20)).await.unwrap();
        wait_for_green(&handle, "dimmed", |g| g < full * 0.5).await;
        let during = handle.snapshot().borrow().clone();
        assert_eq!(during.revision, before.revision, "nothing saved");
        assert_eq!(during.settings, before.settings);
        assert_eq!(store.saved(), Some(settings()));

        // Saving the very settings already saved still ends the trial: the
        // lights follow the saved brightness again.
        let revision = handle
            .replace_settings(before.revision, settings())
            .await
            .unwrap();
        assert_eq!(revision, before.revision.next());
        wait_for_green(&handle, "full again", |g| g > full * 0.9).await;
        assert!(handle.snapshot().borrow().failure.is_none());
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn a_trial_is_validated_like_settings() {
    within(async {
        let store = MemoryStore::with(settings());
        let (handle, engine) = start(&store).await;
        let error = handle.try_processing(brightness(101)).await.unwrap_err();
        assert!(
            matches!(error, Error::Invalid(ref e) if e.field == "processing.brightness_percent")
        );
        engine.shutdown().await.unwrap();
    })
    .await;
}

/// Not under [`within`]: its deadline would expire with the paused clock.
#[tokio::test]
async fn a_trial_nobody_saves_expires() {
    let store = MemoryStore::with(settings());
    let (handle, engine) = start(&store).await;
    handle.set_target(LightingTarget::Processed).await.unwrap();
    let full = wait_for_green(&handle, "lit", |g| g > 0.0).await;
    let second = Duration::from_secs(1);

    tokio::time::pause();
    handle.try_processing(brightness(20)).await.unwrap();
    tokio::time::advance(TRIAL_TIMEOUT - second).await;
    // A no-op command is a round trip through the engine's loop.
    handle.set_target(LightingTarget::Processed).await.unwrap();
    tokio::time::resume();
    wait_for_green(&handle, "dimmed", |g| g < full * 0.5).await;

    // Trying again, even unchanged, restarts the timeout.
    tokio::time::pause();
    handle.try_processing(brightness(20)).await.unwrap();
    tokio::time::advance(TRIAL_TIMEOUT - second).await;
    handle.set_target(LightingTarget::Processed).await.unwrap();
    tokio::time::resume();
    wait_for_green(&handle, "still dimmed", |g| g < full * 0.5).await;

    tokio::time::pause();
    tokio::time::advance(second * 2).await;
    handle.set_target(LightingTarget::Processed).await.unwrap();
    tokio::time::resume();
    wait_for_green(&handle, "full again", |g| g > full * 0.9).await;
    assert_eq!(store.saved(), Some(settings()));
    engine.shutdown().await.unwrap();
}
