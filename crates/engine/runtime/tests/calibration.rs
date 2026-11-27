//! Calibration patterns: engine state beside the target, never saved. While
//! one shows, processing runs without smoothing and, when `uncorrected`,
//! without the active calibration; settings stay as saved.
//!
//! The fake source fills its frames with gray, so the correction shows in the
//! preview: equal red and blue is neutral, less red than blue is the cool
//! calibration. Smoothing is the longest (2 s) and blends frame content (before the
//! correction), so a source blinking between two grays shows only those two
//! levels while a pattern switches smoothing off.
use prysm_ports::{
    AdapterSettings, Calibration, CalibrationPattern, CorrectionControls, Error, LightingTarget,
    Pattern, Processing, Registry, SessionState, Settings, Style, StyleValues,
};
use prysm_runtime::fakes::{
    CallLog, FakeOutputSettings, FakeOutputs, FakeSourceSettings, FakeSources, MemoryStore,
};
use prysm_runtime::{CALIBRATION_TIMEOUT, Engine, EngineHandle};
use prysm_testkit::{eventually_async, within_async};
use std::time::Duration;

const DEADLINE: Duration = Duration::from_secs(2);

/// How long a test may take.
const LIMIT: Duration = Duration::from_secs(20);

fn source(fill: u8, end_after: Option<u32>) -> FakeSourceSettings {
    FakeSourceSettings {
        name: "a".into(),
        fill,
        end_after,
        ..FakeSourceSettings::default()
    }
}

/// Gray frames, the longest smoothing, a cool calibration.
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
            // Slow and without cuts, so only the pattern's own config is quick.
            style: Style::Custom,
            custom_style: Some(StyleValues {
                smoothing_seconds: StyleValues::MAX_SMOOTHING_SECONDS,
                cut_response_percent: 0,
                ..StyleValues::DEFAULT
            }),
            calibrations: vec![Calibration {
                id: "cool".into(),
                name: "Cool".into(),
                correction: CorrectionControls {
                    warmth: -100,
                    ..CorrectionControls::default()
                },
            }],
            active_calibration: active.map(Into::into),
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

const fn pattern(pattern: Pattern, uncorrected: bool) -> Option<CalibrationPattern> {
    Some(CalibrationPattern {
        pattern,
        uncorrected,
    })
}

fn calibration(handle: &EngineHandle) -> Option<CalibrationPattern> {
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
    eventually_async(what, Duration::from_secs(5), || {
        cast(handle).filter(|&cast| done(cast)).map(drop)
    })
    .await;
}

fn neutral(cast: f32) -> bool {
    cast.abs() < 1e-4
}

fn cool(cast: f32) -> bool {
    cast < -1e-4
}

#[tokio::test]
async fn patterns_need_screen_sync() {
    within_async(LIMIT, async {
        let store = MemoryStore::with(settings(None, &source(128, None)));
        let (handle, engine, log) = start(&store).await;
        for target in [LightingTarget::Off, LightingTarget::Effects] {
            handle.set_target(target).await.unwrap();
            let error = handle
                .set_calibration(pattern(Pattern::Red, false))
                .await
                .unwrap_err();
            assert!(matches!(error, Error::NeedsScreenSync), "{error:?}");
            assert_eq!(calibration(&handle), None);
            handle.set_calibration(None).await.unwrap();
        }
        assert_eq!(log.count("source.open:a"), 0, "a pattern starts nothing");
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn set_switch_and_clear_leave_settings_and_revision_untouched() {
    within_async(LIMIT, async {
        let saved = settings(Some("cool"), &source(128, None));
        let store = MemoryStore::with(saved.clone());
        let (handle, engine, log) = start(&store).await;
        let revision = handle.snapshot().borrow().revision;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
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
        assert!(snapshot.session.failure().is_none());
        assert_eq!(store.saved(), Some(saved));
        assert_eq!(log.count("source.open:a"), 1, "patterns never reopen");
        assert_eq!(log.count("output.open:a"), 1);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn uncorrected_shows_at_once_without_the_calibration_and_clearing_restores_it() {
    within_async(LIMIT, async {
        let store = MemoryStore::with(settings(Some("cool"), &source(128, None)));
        let (handle, engine, _) = start(&store).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        wait_for_cast(&handle, "cool", cool).await;

        handle
            .set_calibration(pattern(Pattern::Gray192, true))
            .await
            .unwrap();
        wait_for_cast(&handle, "neutral", neutral).await;

        handle
            .set_calibration(pattern(Pattern::Gray192, false))
            .await
            .unwrap();
        wait_for_cast(&handle, "cool", cool).await;

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
    within_async(LIMIT, async {
        let blinking = FakeSourceSettings {
            blink: Some(64),
            ..source(160, None)
        };
        let saved = settings(Some("cool"), &blinking);
        let store = MemoryStore::with(saved.clone());
        let (handle, engine, log) = start(&store).await;
        let _viewer = handle.preview();
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        wait_for_cast(&handle, "cool", cool).await;
        assert!(green_levels(&handle).await > 2, "long smoothing blends");

        handle
            .set_calibration(pattern(Pattern::Gray192, false))
            .await
            .unwrap();
        // A frame blended before the pattern took effect can still be in the
        // preview: wait for 300 ms without one (`within` bounds the wait).
        // With smoothing on, no 300 ms shows only the two grays.
        while green_levels(&handle).await != 2 {}
        assert!(
            cool(cast(&handle).unwrap()),
            "the calibration still applies"
        );

        handle.set_calibration(None).await.unwrap();
        while green_levels(&handle).await <= 2 {}
        assert_eq!(handle.snapshot().borrow().settings, saved);
        assert_eq!(log.count("source.open:a"), 1);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn leaving_screen_sync_ending_and_shutdown_clear_the_pattern() {
    within_async(LIMIT, async {
        let store = MemoryStore::with(settings(None, &source(128, None)));
        let (handle, engine, _) = start(&store).await;
        for target in [LightingTarget::Off, LightingTarget::Effects] {
            handle.set_target(LightingTarget::ScreenSync).await.unwrap();
            handle
                .snapshot()
                .wait_for(|s| matches!(s.session, SessionState::ScreenSync))
                .await
                .unwrap();
            handle
                .set_calibration(pattern(Pattern::Coast, false))
                .await
                .unwrap();
            handle.set_target(LightingTarget::ScreenSync).await.unwrap();
            assert_eq!(calibration(&handle), pattern(Pattern::Coast, false));
            handle.set_target(target).await.unwrap();
            assert_eq!(calibration(&handle), None, "{target:?}");
        }
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        handle
            .set_calibration(pattern(Pattern::Dusk, true))
            .await
            .unwrap();
        engine.shutdown().await.unwrap();
        assert_eq!(calibration(&handle), None, "cleared at shutdown");

        let store = MemoryStore::with(settings(None, &source(128, Some(500))));
        let (handle, engine, _) = start(&store).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
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

/// Not under [`within_async`]: its deadline would expire with the paused clock.
#[tokio::test]
async fn a_pattern_nobody_sets_again_expires() {
    {
        let store = MemoryStore::with(settings(None, &source(128, None)));
        let (handle, engine, _) = start(&store).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        let minute = Duration::from_secs(60);
        assert!(CALIBRATION_TIMEOUT > minute * 2);

        tokio::time::pause();
        handle
            .set_calibration(pattern(Pattern::Blue, false))
            .await
            .unwrap();
        tokio::time::advance(CALIBRATION_TIMEOUT - minute).await;
        // A no-op command is a round trip through the engine's loop.
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert_eq!(calibration(&handle), pattern(Pattern::Blue, false));

        handle
            .set_calibration(pattern(Pattern::Blue, false))
            .await
            .unwrap();
        tokio::time::advance(CALIBRATION_TIMEOUT - minute).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
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
            LightingTarget::ScreenSync,
            "lighting stays on"
        );
        assert!(snapshot.session.failure().is_none());
        engine.shutdown().await.unwrap();
    }
}
