//! Calibrations: the active calibration's correction reaches the processor, a
//! calibration change while lighting is a frame-boundary hot-swap, and invalid
//! calibrations never reach the store.
//!
//! The fake source fills its frames with gray (`fill`), so the correction is
//! observable in the preview colors: without correction red equals blue, a
//! cool calibration (warmth -100) attenuates red, a warm one (warmth 100) blue.
use prysm_ports::{
    AdapterSettings, Calibration, CorrectionControls, Error, LightingTarget, MAX_CALIBRATIONS,
    Processing, Registry, Settings, SettingsStore, Style, StyleValues,
};
use prysm_runtime::Engine;
use prysm_runtime::EngineHandle;
use prysm_runtime::fakes::{
    CallLog, FakeOutputSettings, FakeOutputs, FakeSourceSettings, FakeSources, MemoryStore,
};
use prysm_testkit::{eventually_async, within_async};
use std::time::Duration;

const DEADLINE: Duration = Duration::from_secs(2);

/// How long a test may take.
const LIMIT: Duration = Duration::from_secs(10);

fn calibration(id: &str, warmth: i16) -> Calibration {
    Calibration {
        id: id.into(),
        name: id.to_uppercase(),
        correction: CorrectionControls {
            warmth,
            ..CorrectionControls::default()
        },
    }
}

/// Gray fake frames, no smoothing, a cool and a warm calibration.
fn with_active(active: Option<&str>) -> Settings {
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
            calibrations: vec![calibration("cool", -100), calibration("warm", 100)],
            active_calibration: active.map(Into::into),
            ..Processing::default()
        },
    }
}

async fn start(log: &CallLog, store: &MemoryStore) -> (EngineHandle, Engine) {
    let mut registry = Registry::default();
    registry.add_source(FakeSources(log.clone()));
    registry.add_output(FakeOutputs(log.clone()));
    Engine::start(registry, Box::new(store.clone()), DEADLINE)
        .await
        .unwrap()
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Cast {
    Neutral,
    Cool,
    Warm,
}

/// Wait up to five seconds for the preview to show `cast`.
async fn wait_for_cast(handle: &EngineHandle, cast: Cast) {
    let preview = handle.preview();
    eventually_async(
        &format!("a {cast:?} preview"),
        Duration::from_secs(5),
        || {
            let color = preview.latest()?.colors.top.sample_at(0.5);
            assert!(color.g > 0.0, "the gray frame lights the LEDs");
            let now = if (color.r - color.b).abs() < 1e-4 {
                Cast::Neutral
            } else if color.r < color.b {
                Cast::Cool
            } else {
                Cast::Warm
            };
            (now == cast).then_some(())
        },
    )
    .await;
}

fn revision(handle: &EngineHandle) -> prysm_ports::Revision {
    handle.snapshot().borrow().revision
}

#[tokio::test]
async fn active_calibration_is_applied_at_start_and_none_is_neutral() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let store = MemoryStore::with(with_active(Some("cool")));
        let (handle, engine) = start(&log, &store).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        wait_for_cast(&handle, Cast::Cool).await;
        engine.shutdown().await.unwrap();

        let store = MemoryStore::with(with_active(None));
        let (handle, engine) = start(&log, &store).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        wait_for_cast(&handle, Cast::Neutral).await;
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn switching_the_active_calibration_while_running_keeps_source_and_output_open() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let store = MemoryStore::with(with_active(Some("cool")));
        let (handle, engine) = start(&log, &store).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        wait_for_cast(&handle, Cast::Cool).await;

        let warm = with_active(Some("warm"));
        let replaced = handle
            .replace_settings(revision(&handle), warm.clone())
            .await
            .unwrap();
        wait_for_cast(&handle, Cast::Warm).await;

        // Editing the active calibration's values is the same hot-swap.
        let mut cooled = warm;
        cooled.processing.calibrations[1].correction.warmth = -100;
        handle
            .replace_settings(replaced, cooled.clone())
            .await
            .unwrap();
        wait_for_cast(&handle, Cast::Cool).await;

        // Clearing the active calibration removes the correction.
        let cleared = with_active(None);
        handle
            .replace_settings(revision(&handle), cleared.clone())
            .await
            .unwrap();
        wait_for_cast(&handle, Cast::Neutral).await;

        assert_eq!(log.count("source.open:a"), 1);
        assert_eq!(log.count("output.open:a"), 1);
        assert_eq!(log.count("source.close:a"), 0);
        assert_eq!(log.count("output.idle:a:Off"), 0);
        assert_eq!(store.saved(), Some(cleared));
        engine.shutdown().await.unwrap();
        assert_eq!(log.count("source.close:a"), 1);
    })
    .await;
}

#[tokio::test]
async fn calibration_change_while_off_waits_for_the_next_start() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let store = MemoryStore::with(with_active(Some("cool")));
        let (handle, engine) = start(&log, &store).await;
        handle
            .replace_settings(revision(&handle), with_active(Some("warm")))
            .await
            .unwrap();
        assert!(log.events().is_empty());

        handle.set_target(LightingTarget::Processed).await.unwrap();
        wait_for_cast(&handle, Cast::Warm).await;
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn invalid_calibrations_are_rejected_before_persistence() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let initial = with_active(Some("cool"));
        let store = MemoryStore::with(initial.clone());
        let (handle, engine) = start(&log, &store).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        wait_for_cast(&handle, Cast::Cool).await;
        let current = revision(&handle);

        let mut duplicate = initial.clone();
        duplicate.processing.calibrations[1].id = "cool".into();
        let mut unknown_active = initial.clone();
        unknown_active.processing.active_calibration = Some("gone".into());
        let mut out_of_range = initial.clone();
        out_of_range.processing.calibrations[1].correction.blue.hue = 101;
        let mut too_many = initial.clone();
        too_many.processing.calibrations = (0..=MAX_CALIBRATIONS)
            .map(|i| calibration(&format!("c{i}"), 0))
            .collect();
        too_many.processing.active_calibration = None;
        let mut bad_name = initial.clone();
        bad_name.processing.calibrations[0].name = " padded".into();
        let mut bad_id = initial.clone();
        bad_id.processing.calibrations[1].id = "no spaces".into();

        for (settings, field) in [
            (duplicate, "processing.calibrations[1].id"),
            (unknown_active, "processing.active_calibration"),
            (
                out_of_range,
                "processing.calibrations[1].correction.blue.hue",
            ),
            (too_many, "processing.calibrations"),
            (bad_name, "processing.calibrations[0].name"),
            (bad_id, "processing.calibrations[1].id"),
        ] {
            let error = handle
                .replace_settings(current, settings)
                .await
                .unwrap_err();
            assert!(
                matches!(error, Error::Invalid(ref e) if e.field == field),
                "expected {field}, got {error:?}"
            );
        }

        assert_eq!(store.saved(), Some(initial));
        assert_eq!(revision(&handle), current);
        assert!(handle.snapshot().borrow().failure.is_none());
        assert_eq!(log.count("source.open:a"), 1);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn calibrations_round_trip_through_the_store_and_a_restart() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let store = MemoryStore::new();
        let (handle, engine) = start(&log, &store).await;
        let saved = with_active(Some("warm"));
        handle
            .replace_settings(revision(&handle), saved.clone())
            .await
            .unwrap();
        engine.shutdown().await.unwrap();
        assert_eq!(store.saved(), Some(saved.clone()));

        let (handle, engine) = start(&log, &store).await;
        assert_eq!(handle.snapshot().borrow().settings, saved);
        assert!(handle.snapshot().borrow().failure.is_none());
        assert_eq!(store.clone().load().unwrap(), Some(saved));
        handle.set_target(LightingTarget::Processed).await.unwrap();
        wait_for_cast(&handle, Cast::Warm).await;
        engine.shutdown().await.unwrap();
    })
    .await;
}
