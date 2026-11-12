//! Calibration patterns: engine state beside the target, never saved. While
//! one shows, processing runs without smoothing and, when `uncorrected`,
//! without the active profile; settings stay as saved.
//!
//! The fake source fills its frames with gray, so the correction shows in the
//! preview: equal red and blue is neutral, less red than blue is the cool
//! profile. Smoothing is long (60 s) and blends frame content (before the
//! correction), so a source blinking between two grays shows only those two
//! levels while a pattern switches smoothing off.
use prysm_ports::{
    AdapterSettings, Calibration, ColorProfile, CorrectionControls, Error, LightingTarget, Pattern,
    Processing, Registry, Settings,
};
use prysm_runtime::fakes::{
    CallLog, FakeOutputSettings, FakeOutputs, FakeSourceSettings, FakeSources, MemoryStore,
};
use prysm_runtime::{CALIBRATION_TIMEOUT, Engine, EngineHandle};
use std::future::Future;
use std::time::Duration;

const DEADLINE: Duration = Duration::from_secs(2);

async fn within<T>(test: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(20), test)
        .await
        .expect("test timed out")
}

fn source(fill: u8, end_after: Option<u32>) -> FakeSourceSettings {
    FakeSourceSettings {
        name: "a".into(),
        fill,
        end_after,
        ..FakeSourceSettings::default()
    }
}

/// Gray frames, 60 s smoothing, a cool profile.
fn settings(active: Option<&str>, source: &FakeSourceSettings) -> Settings {
    Settings {
        capture: Some(AdapterSettings::new("fake", source)),
        output: Some(AdapterSettings::new(
            "fake",
            &FakeOutputSettings {
                name: "a".into(),
                ..FakeOutputSettings::default()
            },
        )),
        processing: Processing {
            smoothing_seconds: 60.0,
            profiles: vec![ColorProfile {
                id: "cool".into(),
                name: "Cool".into(),
                correction: CorrectionControls {
                    warmth: -100,
                    ..CorrectionControls::default()
                },
            }],
            active_profile: active.map(Into::into),
            ..Processing::default()
        },
    }
}

async fn start(store: &MemoryStore) -> (EngineHandle, Engine, CallLog) {
    let log = CallLog::new();
    let mut registry = Registry::default();
    registry.add_source(FakeSources(log.clone()));
    registry.add_output(FakeOutputs(log.clone()));
    let (handle, engine) = Engine::start(registry, Box::new(store.clone()), DEADLINE)
        .await
        .unwrap();
    (handle, engine, log)
}

const fn pattern(pattern: Pattern, uncorrected: bool) -> Option<Calibration> {
    Some(Calibration {
        pattern,
        uncorrected,
    })
}

fn calibration(handle: &EngineHandle) -> Option<Calibration> {
    handle.snapshot().borrow().calibration
}

/// Red minus blue of the latest preview's top center.
fn cast(handle: &EngineHandle) -> Option<f32> {
    let latest = handle.preview().latest()?;
    let color = latest.colors.top.sample_at(0.5);
    assert!(color.g > 0.0, "the gray frame lights the LEDs");
    Some(color.r - color.b)
}

/// Wait up to five seconds for the preview's cast to satisfy `done`.
async fn wait_for_cast(handle: &EngineHandle, what: &str, done: impl Fn(f32) -> bool) {
    let _viewer = handle.preview();
    let mut seen = None;
    for _ in 0..1000 {
        if let Some(cast) = cast(handle) {
            if done(cast) {
                return;
            }
            seen = Some(cast);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("preview never became {what}; last red - blue {seen:?}");
}

fn neutral(cast: f32) -> bool {
    cast.abs() < 1e-4
}

fn cool(cast: f32) -> bool {
    cast < -1e-4
}

#[tokio::test]
async fn patterns_need_processed_lighting() {
    within(async {
        let store = MemoryStore::with(settings(None, &source(128, None)));
        let (handle, engine, log) = start(&store).await;
        for target in [LightingTarget::Off, LightingTarget::DeviceNative] {
            handle.set_target(target).await.unwrap();
            let error = handle
                .set_calibration(pattern(Pattern::Red, false))
                .await
                .unwrap_err();
            assert!(
                matches!(error, Error::Invalid(ref e) if e.field == "target"),
                "{error:?}"
            );
            assert_eq!(calibration(&handle), None);
            // Clearing is always accepted.
            handle.set_calibration(None).await.unwrap();
        }
        assert_eq!(log.count("source.open:a"), 0, "a pattern starts nothing");
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn set_switch_and_clear_leave_settings_and_revision_untouched() {
    within(async {
        let saved = settings(Some("cool"), &source(128, None));
        let store = MemoryStore::with(saved.clone());
        let (handle, engine, log) = start(&store).await;
        let revision = handle.snapshot().borrow().revision;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        wait_for_cast(&handle, "cool", cool).await;

        handle
            .set_calibration(pattern(Pattern::Gray192, false))
            .await
            .unwrap();
        assert_eq!(calibration(&handle), pattern(Pattern::Gray192, false));
        handle
            .set_calibration(pattern(Pattern::ReviewGrid, false))
            .await
            .unwrap();
        assert_eq!(calibration(&handle), pattern(Pattern::ReviewGrid, false));
        handle.set_calibration(None).await.unwrap();
        assert_eq!(calibration(&handle), None);

        let snapshot = handle.snapshot().borrow().clone();
        assert_eq!(snapshot.revision, revision);
        assert_eq!(snapshot.settings, saved);
        assert!(snapshot.failure.is_none());
        assert_eq!(store.saved(), Some(saved));
        assert_eq!(log.count("source.open:a"), 1, "patterns never reopen");
        assert_eq!(log.count("output.open:a"), 1);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn uncorrected_shows_at_once_without_the_profile_and_clearing_restores_it() {
    within(async {
        let store = MemoryStore::with(settings(Some("cool"), &source(128, None)));
        let (handle, engine, _) = start(&store).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        wait_for_cast(&handle, "cool", cool).await;

        // Uncorrected: no profile and no smoothing, so exactly neutral at once.
        handle
            .set_calibration(pattern(Pattern::Gray192, true))
            .await
            .unwrap();
        wait_for_cast(&handle, "neutral", neutral).await;

        // Corrected again: the profile is back.
        handle
            .set_calibration(pattern(Pattern::Gray192, false))
            .await
            .unwrap();
        wait_for_cast(&handle, "cool", cool).await;

        // Uncorrected, then cleared: the profile and its smoothing return.
        handle
            .set_calibration(pattern(Pattern::Gray192, true))
            .await
            .unwrap();
        wait_for_cast(&handle, "neutral", neutral).await;
        handle.set_calibration(None).await.unwrap();
        wait_for_cast(&handle, "cool", cool).await;
        engine.shutdown().await.unwrap();
    })
    .await;
}

/// How many different green levels the preview shows over 300 ms of a source
/// blinking between two grays: two without smoothing, many with it.
async fn green_levels(handle: &EngineHandle) -> usize {
    let mut levels = std::collections::BTreeSet::new();
    for _ in 0..60 {
        if let Some(latest) = handle.preview().latest() {
            levels.insert(latest.colors.top.sample_at(0.5).g.to_bits());
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    levels.len()
}

#[tokio::test]
async fn smoothing_is_off_while_a_pattern_shows_and_returns_after() {
    within(async {
        let blinking = FakeSourceSettings {
            blink: Some(64),
            ..source(160, None)
        };
        let saved = settings(Some("cool"), &blinking);
        let store = MemoryStore::with(saved.clone());
        let (handle, engine, log) = start(&store).await;
        let _viewer = handle.preview();
        handle.set_target(LightingTarget::Processed).await.unwrap();
        wait_for_cast(&handle, "cool", cool).await;
        assert!(green_levels(&handle).await > 2, "60 s smoothing blends");

        handle
            .set_calibration(pattern(Pattern::Gray192, false))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(green_levels(&handle).await, 2, "no smoothing");
        assert!(cool(cast(&handle).unwrap()), "the profile still applies");

        handle.set_calibration(None).await.unwrap();
        // The restarted smoother passes its first frame unblended.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(green_levels(&handle).await > 2, "smoothing returns");
        assert_eq!(handle.snapshot().borrow().settings, saved);
        assert_eq!(log.count("source.open:a"), 1);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn leaving_processed_ending_and_shutdown_clear_the_pattern() {
    within(async {
        let store = MemoryStore::with(settings(None, &source(128, None)));
        let (handle, engine, _) = start(&store).await;
        for target in [LightingTarget::Off, LightingTarget::DeviceNative] {
            handle.set_target(LightingTarget::Processed).await.unwrap();
            handle
                .set_calibration(pattern(Pattern::Coast, false))
                .await
                .unwrap();
            // Asking for processed again keeps it.
            handle.set_target(LightingTarget::Processed).await.unwrap();
            assert_eq!(calibration(&handle), pattern(Pattern::Coast, false));
            handle.set_target(target).await.unwrap();
            assert_eq!(calibration(&handle), None, "{target:?}");
        }
        handle.set_target(LightingTarget::Processed).await.unwrap();
        handle
            .set_calibration(pattern(Pattern::Dusk, true))
            .await
            .unwrap();
        engine.shutdown().await.unwrap();
        assert_eq!(calibration(&handle), None, "cleared at shutdown");

        // A source that ends turns lighting off, and with it the pattern.
        let store = MemoryStore::with(settings(None, &source(128, Some(500))));
        let (handle, engine, _) = start(&store).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        handle
            .set_calibration(pattern(Pattern::Garden, false))
            .await
            .unwrap();
        let ended = handle
            .snapshot()
            .wait_for(|s| s.target == LightingTarget::Off)
            .await
            .unwrap()
            .clone();
        assert_eq!(ended.calibration, None);
        engine.shutdown().await.unwrap();
    })
    .await;
}

/// Not under [`within`]: its deadline would expire with the paused clock.
#[tokio::test]
async fn a_pattern_nobody_sets_again_expires() {
    {
        let store = MemoryStore::with(settings(None, &source(128, None)));
        let (handle, engine, _) = start(&store).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        let minute = Duration::from_secs(60);
        assert!(CALIBRATION_TIMEOUT > minute * 2);

        tokio::time::pause();
        handle
            .set_calibration(pattern(Pattern::Blue, false))
            .await
            .unwrap();
        tokio::time::advance(CALIBRATION_TIMEOUT - minute).await;
        // A no-op command is a round trip through the engine's loop.
        handle.set_target(LightingTarget::Processed).await.unwrap();
        assert_eq!(calibration(&handle), pattern(Pattern::Blue, false));

        // Setting it again, even unchanged, restarts the timeout.
        handle
            .set_calibration(pattern(Pattern::Blue, false))
            .await
            .unwrap();
        tokio::time::advance(CALIBRATION_TIMEOUT - minute).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        assert_eq!(calibration(&handle), pattern(Pattern::Blue, false));

        tokio::time::advance(minute * 2).await;
        handle
            .snapshot()
            .wait_for(|s| s.calibration.is_none())
            .await
            .unwrap();
        tokio::time::resume();
        let snapshot = handle.snapshot().borrow().clone();
        assert_eq!(
            snapshot.target,
            LightingTarget::Processed,
            "lighting stays on"
        );
        assert!(snapshot.failure.is_none());
        engine.shutdown().await.unwrap();
    }
}
