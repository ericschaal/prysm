//! Tried processing: a control being dragged lights with its value at once,
//! without saving it or changing the revision; a save or a timeout ends it.
//!
//! The fake source fills its frames with gray and smoothing is off, so the
//! brightness shows in the preview's green at once.
use prysm_ports::{
    AdapterSettings, Error, LightingTarget, Processing, Registry, Settings, Style, StyleValues,
};
use prysm_runtime::fakes::{
    CallLog, FakeOutputSettings, FakeOutputs, FakeSourceSettings, FakeSources, MemoryStore,
};
use prysm_runtime::{Engine, EngineHandle, TRIAL_TIMEOUT};
use prysm_testkit::{eventually_async, within_async};
use std::time::Duration;

const DEADLINE: Duration = Duration::from_secs(2);

/// How long a test may take.
const LIMIT: Duration = Duration::from_secs(20);

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
            style: Style::Custom,
            custom_style: Some(StyleValues {
                smoothing_seconds: 0.0,
                ..StyleValues::DEFAULT
            }),
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
    let preview = handle.preview();
    eventually_async(what, Duration::from_secs(5), || {
        let green = preview.latest()?.colors.top.sample_at(0.5).g;
        done(green).then_some(green)
    })
    .await
}

#[tokio::test]
async fn a_trial_lights_without_saving_and_a_save_ends_it() {
    within_async(LIMIT, async {
        let store = MemoryStore::with(settings());
        let (handle, engine) = start(&store).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        let full = wait_for_green(&handle, "lit", |g| g > 0.0).await;
        let before = handle.snapshot().borrow().clone();

        handle.try_processing(brightness(20)).await.unwrap();
        wait_for_green(&handle, "dimmed", |g| g < full * 0.5).await;
        let during = handle.snapshot().borrow().clone();
        assert_eq!(during.revision, before.revision, "nothing saved");
        assert_eq!(during.settings, before.settings);
        assert_eq!(store.saved(), Some(settings()));

        let revision = handle
            .replace_settings(before.revision, settings())
            .await
            .unwrap();
        assert_eq!(revision, before.revision.next());
        wait_for_green(&handle, "full again", |g| g > full * 0.9).await;
        assert!(handle.snapshot().borrow().session.failure().is_none());
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn a_trial_is_validated_like_settings() {
    within_async(LIMIT, async {
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

/// Not under [`within_async`]: its deadline would expire with the paused clock.
#[tokio::test]
async fn a_trial_nobody_saves_expires() {
    let store = MemoryStore::with(settings());
    let (handle, engine) = start(&store).await;
    handle.set_target(LightingTarget::ScreenSync).await.unwrap();
    let full = wait_for_green(&handle, "lit", |g| g > 0.0).await;
    let second = Duration::from_secs(1);

    tokio::time::pause();
    handle.try_processing(brightness(20)).await.unwrap();
    tokio::time::advance(TRIAL_TIMEOUT - second).await;
    // A no-op command is a round trip through the engine's loop.
    handle.set_target(LightingTarget::ScreenSync).await.unwrap();
    tokio::time::resume();
    wait_for_green(&handle, "dimmed", |g| g < full * 0.5).await;

    tokio::time::pause();
    handle.try_processing(brightness(20)).await.unwrap();
    tokio::time::advance(TRIAL_TIMEOUT - second).await;
    handle.set_target(LightingTarget::ScreenSync).await.unwrap();
    tokio::time::resume();
    wait_for_green(&handle, "still dimmed", |g| g < full * 0.5).await;

    tokio::time::pause();
    tokio::time::advance(second * 2).await;
    handle.set_target(LightingTarget::ScreenSync).await.unwrap();
    tokio::time::resume();
    wait_for_green(&handle, "full again", |g| g > full * 0.9).await;
    assert_eq!(store.saved(), Some(settings()));
    engine.shutdown().await.unwrap();
}
