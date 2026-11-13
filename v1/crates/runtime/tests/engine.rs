use prysm_ports::{
    AdapterSettings, Behind, Error, Latency, LightingTarget, OutputIssue, Processing, Registry,
    Revision, Section, SessionPhase, SessionState, Settings, Snapshot,
};
use prysm_runtime::fakes::{
    CallLog, FakeOutputSettings, FakeOutputs, FakeSourceSettings, FakeSources, MemoryStore,
};
use prysm_runtime::{ColorsWatch, Engine, EngineHandle, Prysm, Shutdown};
use std::future::Future;
use std::time::Duration;

const DEADLINE: Duration = Duration::from_secs(2);

async fn within<T>(test: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), test)
        .await
        .expect("test timed out")
}

fn settings(source: FakeSourceSettings, output: FakeOutputSettings) -> Settings {
    Settings {
        capture: Some(AdapterSettings::new("fake", &source)),
        output: Some(AdapterSettings::new("fake", &output)),
        processing: Processing::default(),
    }
}

fn named(source: &str, output: &str) -> Settings {
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

fn registry(log: &CallLog) -> Registry {
    let mut registry = Registry::default();
    registry.add_source(FakeSources(log.clone()));
    registry.add_output(FakeOutputs(log.clone()));
    registry
}

async fn start(
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

async fn snapshot_until(handle: &EngineHandle, done: impl FnMut(&Snapshot) -> bool) -> Snapshot {
    handle.snapshot().wait_for(done).await.unwrap().clone()
}

fn revision(handle: &EngineHandle) -> Revision {
    handle.snapshot().borrow().revision
}

fn before(log: &CallLog, earlier: &str, later: &str) -> bool {
    match (log.last_position(earlier), log.position(later)) {
        (Some(earlier), Some(later)) => earlier < later,
        _ => false,
    }
}

#[tokio::test]
async fn stop_writes_then_closes_source_then_idles_output_then_joins() {
    within(async {
        let log = CallLog::new();
        let (handle, engine, _) = start(&log, named("a", "a"), DEADLINE).await;
        let preview = handle.preview();
        handle.set_target(LightingTarget::Processed).await.unwrap();
        assert!(log.wait_for("output.write:a", 3).await);
        assert!(preview.latest().is_some());

        handle.set_target(LightingTarget::Off).await.unwrap();
        log.record("joined");

        assert!(before(&log, "output.write:a", "source.close:a"));
        assert!(before(&log, "source.close:a", "output.idle:a:Off"));
        assert!(before(&log, "output.idle:a:Off", "joined"));
        let snapshot = handle.snapshot().borrow().clone();
        assert_eq!(snapshot.session, SessionState::Stopped);
        assert!(snapshot.failure.is_none());
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn output_idles_when_the_source_fails() {
    within(async {
        let log = CallLog::new();
        let failing = settings(
            FakeSourceSettings {
                name: "a".into(),
                fail_after: Some(3),
                ..Default::default()
            },
            FakeOutputSettings {
                name: "a".into(),
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, failing, DEADLINE).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();

        let snapshot = snapshot_until(&handle, |s| s.failure.is_some()).await;
        assert_eq!(snapshot.session, SessionState::Stopped);
        assert_eq!(snapshot.target, LightingTarget::Processed);
        assert_eq!(log.count("output.write:a"), 3);
        assert!(before(&log, "source.close:a", "output.idle:a:Off"));
        // The failure says it was the capture's.
        assert_eq!(
            snapshot.failure.as_ref().and_then(Error::section),
            Some(Section::Capture)
        );
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn output_idles_when_a_write_fails() {
    within(async {
        let log = CallLog::new();
        let failing = settings(
            FakeSourceSettings {
                name: "a".into(),
                ..Default::default()
            },
            FakeOutputSettings {
                name: "a".into(),
                fail_after: Some(2),
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, failing, DEADLINE).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();

        let snapshot = snapshot_until(&handle, |s| s.failure.is_some()).await;
        assert!(matches!(snapshot.failure, Some(Error::Adapter { .. })));
        assert_eq!(
            snapshot.failure.as_ref().and_then(Error::section),
            Some(Section::Output),
            "the lights' failure"
        );
        assert_eq!(log.count("source.close:a"), 1);
        assert_eq!(log.count("output.idle:a:Off"), 1);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn processing_change_keeps_source_and_output_open() {
    within(async {
        let log = CallLog::new();
        let (handle, engine, store) = start(&log, named("a", "a"), DEADLINE).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        assert!(log.wait_for("output.write:a", 2).await);

        let mut changed = named("a", "a");
        changed.processing.brightness_percent = 40;
        let initial = revision(&handle);
        let replaced = handle
            .replace_settings(initial, changed.clone())
            .await
            .unwrap();
        assert_eq!(replaced, initial.next());
        assert_eq!(revision(&handle), replaced);
        let writes = log.count("output.write:a");
        assert!(log.wait_for("output.write:a", writes + 3).await);

        assert_eq!(log.count("source.open:a"), 1);
        assert_eq!(log.count("output.open:a"), 1);
        assert_eq!(log.count("source.close:a"), 0);
        assert_eq!(store.saved(), Some(changed.clone()));
        assert_eq!(handle.snapshot().borrow().settings, changed);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn adapter_change_restarts_a_running_session() {
    within(async {
        let log = CallLog::new();
        let (handle, engine, _) = start(&log, named("a", "a"), DEADLINE).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        assert!(log.wait_for("output.write:a", 2).await);

        let replaced = handle
            .replace_settings(revision(&handle), named("b", "a"))
            .await
            .unwrap();
        assert!(before(&log, "source.close:a", "output.idle:a:Off"));
        assert!(before(&log, "output.idle:a:Off", "source.open:b"));
        assert!(log.wait_for("output.open:a", 2).await);
        assert_eq!(handle.snapshot().borrow().session, SessionState::Running);

        handle
            .replace_settings(replaced, named("b", "c"))
            .await
            .unwrap();
        assert!(log.wait_for("output.write:c", 1).await);
        assert_eq!(log.count("source.open:b"), 2);
        assert!(log.last_position("source.close:b") < log.last_position("source.open:b"));
        engine.shutdown().await.unwrap();
        assert_eq!(log.count("output.idle:c:Off"), 1);
    })
    .await;
}

#[tokio::test]
async fn adapter_change_while_off_opens_nothing() {
    within(async {
        let log = CallLog::new();
        let (handle, engine, _) = start(&log, named("a", "a"), DEADLINE).await;
        handle
            .replace_settings(revision(&handle), named("b", "b"))
            .await
            .unwrap();
        assert!(log.events().is_empty());
        engine.shutdown().await.unwrap();
        assert!(log.events().is_empty());
    })
    .await;
}

#[tokio::test]
async fn invalid_or_stale_settings_are_rejected_before_persistence() {
    within(async {
        let log = CallLog::new();
        let initial = named("a", "a");
        let (handle, engine, store) = start(&log, initial.clone(), DEADLINE).await;
        let current = revision(&handle);

        let invalid = settings(
            FakeSourceSettings {
                invalid: true,
                ..Default::default()
            },
            FakeOutputSettings::default(),
        );
        let error = handle.replace_settings(current, invalid).await.unwrap_err();
        assert!(matches!(error, Error::Invalid(ref e) if e.field == "capture.invalid"));

        let mut unknown = named("a", "a");
        unknown.output = Some(AdapterSettings::new("nope", &FakeOutputSettings::default()));
        let error = handle.replace_settings(current, unknown).await.unwrap_err();
        assert!(matches!(error, Error::Invalid(ref e) if e.field == "output.kind"));

        let mut out_of_range = named("a", "a");
        out_of_range.processing.brightness_percent = 101;
        assert!(
            handle
                .replace_settings(current, out_of_range)
                .await
                .is_err()
        );

        let stale = Revision(current.0.wrapping_sub(1));
        let error = handle
            .replace_settings(stale, named("b", "b"))
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            Error::StaleRevision { expected, current: now } if expected == stale && now == current
        ));

        assert_eq!(store.saved(), Some(initial));
        assert_eq!(revision(&handle), current);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn restart_failure_keeps_settings_saved_and_a_later_change_recovers() {
    within(async {
        let log = CallLog::new();
        let (handle, engine, store) = start(&log, named("a", "a"), DEADLINE).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        assert!(log.wait_for("output.write:a", 1).await);

        let broken = settings(
            FakeSourceSettings {
                name: "a".into(),
                ..Default::default()
            },
            FakeOutputSettings {
                name: "b".into(),
                fail_open: true,
                ..Default::default()
            },
        );
        let initial = revision(&handle);
        let replaced = handle
            .replace_settings(initial, broken.clone())
            .await
            .unwrap();
        assert_eq!(replaced, initial.next());
        assert_eq!(store.saved(), Some(broken));

        // The output opens at the first frame; its failure ends the session.
        let snapshot = snapshot_until(&handle, |s| s.failure.is_some()).await;
        assert_eq!(
            snapshot
                .failure
                .map(|failure| failure.to_string())
                .as_deref(),
            Some("fake output cannot open")
        );
        assert_eq!(snapshot.session, SessionState::Stopped);
        assert_eq!(snapshot.target, LightingTarget::Processed);
        assert_eq!(snapshot.revision, replaced);
        // The open failure released the newly opened source.
        assert_eq!(log.count("source.close:a"), 2);
        assert_eq!(log.count("output.idle:a:Off"), 1);

        // A later valid change gets the next attempt at the requested target.
        handle
            .replace_settings(replaced, named("a", "c"))
            .await
            .unwrap();
        assert!(log.wait_for("output.write:c", 1).await);
        let snapshot = handle.snapshot().borrow().clone();
        assert_eq!(snapshot.session, SessionState::Running);
        assert!(snapshot.failure.is_none());
        engine.shutdown().await.unwrap();
        assert_eq!(log.count("output.idle:c:Off"), 1);
    })
    .await;
}

#[tokio::test]
async fn missed_stop_deadline_faults_and_only_shutdown_is_accepted() {
    within(async {
        let log = CallLog::new();
        let hanging = settings(
            FakeSourceSettings {
                name: "a".into(),
                close_delay_ms: 3_000,
                ..Default::default()
            },
            FakeOutputSettings {
                name: "a".into(),
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, hanging, Duration::from_millis(100)).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        assert!(log.wait_for("output.write:a", 1).await);

        let error = handle.set_target(LightingTarget::Off).await.unwrap_err();
        assert!(matches!(error, Error::Faulted(SessionPhase::ClosingSource)));
        assert_eq!(
            handle.snapshot().borrow().session,
            SessionState::Faulted(SessionPhase::ClosingSource)
        );
        assert!(matches!(
            handle.set_target(LightingTarget::Processed).await,
            Err(Error::Faulted(_))
        ));
        assert!(matches!(
            handle
                .replace_settings(revision(&handle), named("b", "b"))
                .await,
            Err(Error::Faulted(_))
        ));

        let shutdown = tokio::time::timeout(Duration::from_secs(1), engine.shutdown())
            .await
            .expect("shutdown returns while faulted");
        assert!(matches!(shutdown, Err(Error::Faulted(_))));
    })
    .await;
}

#[tokio::test]
async fn device_native_idles_the_output_without_a_session() {
    within(async {
        let log = CallLog::new();
        let (handle, engine, _) = start(&log, named("a", "a"), DEADLINE).await;
        handle
            .set_target(LightingTarget::DeviceNative)
            .await
            .unwrap();
        assert_eq!(handle.snapshot().borrow().session, SessionState::Native);
        assert_eq!(log.events(), ["output.open:a", "output.idle:a:Native"]);

        handle.set_target(LightingTarget::Off).await.unwrap();
        assert_eq!(log.count("output.idle:a:Off"), 1);
        assert_eq!(log.count("source.open:a"), 0);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn failing_surface_does_not_stop_lighting() {
    within(async {
        let log = CallLog::new();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let controller = {
            let log = log.clone();
            move |engine: EngineHandle, _shutdown: Shutdown| async move {
                engine.set_target(LightingTarget::Processed).await?;
                engine
                    .snapshot()
                    .wait_for(|s| !s.surface_failures.is_empty())
                    .await
                    .map_err(|_| Error::Closed)?;
                let writes = log.count("output.write:a");
                assert!(log.wait_for("output.write:a", writes + 3).await);
                assert_eq!(engine.snapshot().borrow().session, SessionState::Running);
                let _ = stop.send(());
                Ok(())
            }
        };
        let failing = |_engine: EngineHandle, _shutdown: Shutdown| async {
            Err(Error::other("surface broke"))
        };

        Prysm::builder(MemoryStore::with(named("a", "a")))
            .source(FakeSources(log.clone()))
            .output(FakeOutputs(log.clone()))
            .surface(failing)
            .surface(controller)
            .run(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();

        assert!(before(&log, "source.close:a", "output.idle:a:Off"));
    })
    .await;
}

#[tokio::test]
async fn quiet_source_rewrites_last_colors() {
    within(async {
        let log = CallLog::new();
        let source = FakeSourceSettings {
            name: "s".into(),
            quiet_after: Some(2),
            ..Default::default()
        };
        let output = FakeOutputSettings {
            name: "o".into(),
            ..Default::default()
        };
        let mut registry = Registry::default();
        registry.add_source(FakeSources(log.clone()));
        registry.add_output(FakeOutputs(log.clone()));
        let store = MemoryStore::with(settings(source, output));
        let (handle, engine) = Engine::start(registry, Box::new(store), DEADLINE)
            .await
            .unwrap();
        handle.set_target(LightingTarget::Processed).await.unwrap();
        // Two frames, then at least one refresh of the last colors.
        assert!(log.wait_for("output.write:o", 3).await);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn unconfirmed_source_release_faults_the_engine() {
    within(async {
        let log = CallLog::new();
        let source = FakeSourceSettings {
            name: "s".into(),
            fail_close: true,
            ..Default::default()
        };
        let output = FakeOutputSettings {
            name: "o".into(),
            ..Default::default()
        };
        let mut registry = Registry::default();
        registry.add_source(FakeSources(log.clone()));
        registry.add_output(FakeOutputs(log.clone()));
        let store = MemoryStore::with(settings(source, output));
        let (handle, engine) = Engine::start(registry, Box::new(store), DEADLINE)
            .await
            .unwrap();
        handle.set_target(LightingTarget::Processed).await.unwrap();
        assert!(log.wait_for("output.write:o", 1).await);
        assert!(handle.set_target(LightingTarget::Off).await.is_err());
        // The output was still idled, and nothing may reopen the device.
        assert_eq!(log.count("output.idle:o:Off"), 1);
        assert_eq!(
            handle.snapshot().borrow().session,
            SessionState::Faulted(SessionPhase::ClosingSource)
        );
        assert!(matches!(
            handle.set_target(LightingTarget::Processed).await,
            Err(Error::Faulted(SessionPhase::ClosingSource))
        ));
        assert!(engine.shutdown().await.is_err());
    })
    .await;
}

#[tokio::test]
async fn each_engine_starts_at_its_own_revision_so_old_revisions_are_stale() {
    within(async {
        let log = CallLog::new();
        let store = MemoryStore::with(named("a", "a"));
        let (first, engine) = Engine::start(registry(&log), Box::new(store.clone()), DEADLINE)
            .await
            .unwrap();
        let before_restart = revision(&first);
        engine.shutdown().await.unwrap();

        let (second, engine) = Engine::start(registry(&log), Box::new(store.clone()), DEADLINE)
            .await
            .unwrap();
        let current = revision(&second);
        assert_ne!(before_restart, current);
        assert!(current.0 < 1 << 52, "{current} must stay an exact JSON number");
        let error = second
            .replace_settings(before_restart, named("b", "b"))
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            Error::StaleRevision { expected, current: now } if expected == before_restart && now == current
        ));
        assert_eq!(store.saved(), Some(named("a", "a")));
        engine.shutdown().await.unwrap();
    })
    .await;
}

// Scenarios ported from the shipping controller (`apps/prysm/tests/controller.rs`).

#[tokio::test]
async fn output_change_while_off_neither_opens_nor_powers_the_new_output() {
    within(async {
        let log = CallLog::new();
        let (handle, engine, store) = start(&log, named("a", "a"), DEADLINE).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        assert!(log.wait_for("output.write:a", 1).await);
        handle.set_target(LightingTarget::Off).await.unwrap();
        let events = log.events();

        handle
            .replace_settings(revision(&handle), named("a", "b"))
            .await
            .unwrap();
        assert_eq!(store.saved(), Some(named("a", "b")));
        assert_eq!(handle.snapshot().borrow().session, SessionState::Stopped);
        engine.shutdown().await.unwrap();
        // Nothing was opened, written or idled after lighting went off.
        assert_eq!(log.events(), events);
    })
    .await;
}

#[tokio::test]
async fn stop_with_a_failing_idle_still_closes_the_source_and_reports_it() {
    within(async {
        let log = CallLog::new();
        let unreachable = settings(
            FakeSourceSettings {
                name: "a".into(),
                ..Default::default()
            },
            FakeOutputSettings {
                name: "a".into(),
                fail_idle: true,
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, unreachable, DEADLINE).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        assert!(log.wait_for("output.write:a", 1).await);

        let error = handle.set_target(LightingTarget::Off).await.unwrap_err();
        assert_eq!(error.to_string(), "fake output cannot idle");
        assert!(before(&log, "source.close:a", "output.idle:a:Off"));
        let snapshot = handle.snapshot().borrow().clone();
        assert_eq!(snapshot.target, LightingTarget::Off);
        assert_eq!(snapshot.session, SessionState::Stopped);
        assert_eq!(
            snapshot
                .failure
                .map(|failure| failure.to_string())
                .as_deref(),
            Some("fake output cannot idle")
        );

        // Not faulted: lighting can start again, and shutdown reports the idle failure.
        let opened = log.count("output.open:a");
        handle.set_target(LightingTarget::Processed).await.unwrap();
        assert_eq!(log.count("source.open:a"), 2);
        assert!(log.wait_for("output.open:a", opened + 1).await);
        let shutdown = engine.shutdown().await.unwrap_err();
        assert_eq!(shutdown.to_string(), "fake output cannot idle");
        assert_eq!(log.count("source.close:a"), 2);
    })
    .await;
}

#[tokio::test]
async fn concurrent_replacements_of_one_revision_admit_exactly_one() {
    within(async {
        let log = CallLog::new();
        let (handle, engine, store) = start(&log, named("a", "a"), DEADLINE).await;
        let current = revision(&handle);
        let (b, c) = tokio::join!(
            handle.replace_settings(current, named("b", "b")),
            handle.replace_settings(current, named("c", "c")),
        );

        let (winner, loser) = match (b, c) {
            (Ok(revision), Err(error)) => ((revision, named("b", "b")), error),
            (Err(error), Ok(revision)) => ((revision, named("c", "c")), error),
            other => panic!("expected exactly one winner, got {other:?}"),
        };
        assert_eq!(winner.0, current.next());
        assert!(matches!(
            loser,
            Error::StaleRevision { expected, current: now } if expected == current && now == winner.0
        ));
        assert_eq!(store.saved(), Some(winner.1.clone()));
        assert_eq!(handle.snapshot().borrow().settings, winner.1);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn capture_saved_while_processed_is_requested_starts_lighting() {
    within(async {
        let log = CallLog::new();
        let mut output_only = named("a", "a");
        output_only.capture = None;
        let (handle, engine, _) = start(&log, output_only, DEADLINE).await;
        let error = handle
            .set_target(LightingTarget::Processed)
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Invalid(ref e) if e.field == "capture"));
        assert_eq!(handle.snapshot().borrow().target, LightingTarget::Processed);
        assert!(log.events().is_empty());

        handle
            .replace_settings(revision(&handle), named("a", "a"))
            .await
            .unwrap();
        assert!(log.wait_for("output.write:a", 1).await);
        assert_eq!(handle.snapshot().borrow().session, SessionState::Running);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn an_ended_source_stops_cleanly_turns_lighting_off_and_idles_the_output() {
    within(async {
        let log = CallLog::new();
        let ending = settings(
            FakeSourceSettings {
                name: "a".into(),
                end_after: Some(3),
                ..Default::default()
            },
            FakeOutputSettings {
                name: "a".into(),
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, ending, DEADLINE).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();

        let snapshot = snapshot_until(&handle, |s| s.session == SessionState::Stopped).await;
        assert!(snapshot.failure.is_none(), "{:?}", snapshot.failure);
        assert_eq!(snapshot.target, LightingTarget::Off);
        assert!(snapshot.input.is_none());
        assert_eq!(log.count("output.write:a"), 3);
        assert!(before(&log, "output.write:a", "source.close:a"));
        assert!(before(&log, "source.close:a", "output.idle:a:Off"));

        // Off: a settings change does not replay the source.
        let current = revision(&handle);
        let mut changed = handle.snapshot().borrow().settings.clone();
        changed.processing.brightness_percent =
            changed.processing.brightness_percent.saturating_sub(1);
        handle.replace_settings(current, changed).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(log.count("source.open:a"), 1);
        assert_eq!(handle.snapshot().borrow().session, SessionState::Stopped);

        // Asked again, it plays again.
        handle.set_target(LightingTarget::Processed).await.unwrap();
        assert!(log.wait_for("source.open:a", 2).await);
        snapshot_until(&handle, |s| s.target == LightingTarget::Off).await;
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn input_reports_frame_rate_and_quiet_sources_about_once_a_second() {
    within(async {
        let log = CallLog::new();
        let quiet = settings(
            FakeSourceSettings {
                name: "a".into(),
                quiet_after: Some(150),
                ..Default::default()
            },
            FakeOutputSettings::default(),
        );
        let (handle, engine, _) = start(&log, quiet, DEADLINE).await;
        assert!(handle.snapshot().borrow().input.is_none(), "no session");

        handle.set_target(LightingTarget::Processed).await.unwrap();
        let started = tokio::time::Instant::now();
        let waiting = handle.snapshot().borrow().input.unwrap();
        assert!(!waiting.receiving, "nothing measured yet");

        // The fake delivers a frame about every 2 ms.
        let receiving = snapshot_until(&handle, |s| s.input.is_some_and(|i| i.receiving)).await;
        assert!(
            started.elapsed() >= Duration::from_millis(900),
            "sampled per second"
        );
        // The exact rate depends on the machine's load; receiving means at
        // least one frame in the window.
        let fps = receiving.input.unwrap().fps;
        assert!(fps > 0.0, "{fps}");

        // After 150 frames it goes quiet.
        let quiet = snapshot_until(&handle, |s| s.input.is_some_and(|i| !i.receiving)).await;
        assert!(quiet.input.unwrap().fps.abs() < f32::EPSILON);
        assert_eq!(quiet.session, SessionState::Running);

        handle.set_target(LightingTarget::Off).await.unwrap();
        assert!(handle.snapshot().borrow().input.is_none());
        engine.shutdown().await.unwrap();
    })
    .await;
}

/// A source that says when it captured each frame: the span starts there.
#[tokio::test]
async fn latency_starts_at_capture_when_every_frame_has_a_capture_time() {
    within(async {
        let log = CallLog::new();
        let captured = settings(
            FakeSourceSettings {
                name: "a".into(),
                captured_ago_ms: Some(300),
                ..Default::default()
            },
            FakeOutputSettings {
                name: "a".into(),
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, captured, DEADLINE).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        let measured =
            snapshot_until(&handle, |s| s.input.is_some_and(|i| i.latency.is_some())).await;
        let input = measured.input.unwrap();
        let Latency {
            capture_ms,
            output_ms,
        } = input.latency.unwrap();
        // Captured 300 ms before the fake hands it over: that part is before
        // Prysm; the rest is Prysm's own.
        let capture_ms = capture_ms.expect("every frame had a capture time");
        assert!((300.0..400.0).contains(&capture_ms), "{capture_ms}");
        assert!(output_ms < 100.0, "{output_ms}");
        engine.shutdown().await.unwrap();
    })
    .await;
}

/// What reaches the lights: the rate of frames written, and per frame the
/// span from Prysm receiving it to the write for it returning (here a slow
/// device of 20 ms), as a mean over the sample window. Frames whose write
/// fails do not count.
#[tokio::test]
async fn input_reports_frames_sent_and_the_received_to_flushed_span() {
    within(async {
        let log = CallLog::new();
        let slow = settings(
            FakeSourceSettings {
                name: "a".into(),
                ..Default::default()
            },
            FakeOutputSettings {
                name: "a".into(),
                write_delay_ms: 20,
                fail_after: Some(60),
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, slow, DEADLINE).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        let measured =
            snapshot_until(&handle, |s| s.input.is_some_and(|i| i.latency.is_some())).await;
        let input = measured.input.unwrap();
        let Latency {
            capture_ms,
            output_ms,
        } = input.latency.unwrap();
        // At least the write itself; the rest is processing.
        assert!(output_ms >= 20.0, "{output_ms}");
        assert!(output_ms < 200.0, "{output_ms}");
        // Frames without a capture time have no capture part.
        assert_eq!(capture_ms, None);
        // One write at a time, about 50 a second.
        assert!(
            input.sent_fps > 0.0 && input.sent_fps <= 55.0,
            "{}",
            input.sent_fps
        );

        // The 61st write fails: only flushed frames are timed.
        snapshot_until(&handle, |s| s.failure.is_some()).await;
        let stats = handle.stats();
        assert_eq!(stats.flushed.count, stats.frames_sent);
        assert_eq!(stats.frames_sent, 60);
        assert!(stats.frames_received > stats.frames_sent);
        engine.shutdown().await.unwrap();
    })
    .await;
}

/// A device that stops showing the colours is reported, not fought: the
/// session keeps writing, and the report goes with the session.
#[tokio::test]
async fn output_issues_are_reported_while_lighting_keeps_running() {
    within(async {
        let log = CallLog::new();
        let overridden = settings(
            FakeSourceSettings {
                name: "s".into(),
                ..Default::default()
            },
            FakeOutputSettings {
                name: "o".into(),
                overridden_after: Some(20),
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, overridden, DEADLINE).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        let reported = snapshot_until(&handle, |s| s.output_issue.is_some()).await;
        assert_eq!(reported.output_issue, Some(OutputIssue::Overridden));
        assert_eq!(reported.session, SessionState::Running);
        assert!(reported.failure.is_none());
        let writes = log.count("output.write:o");
        assert!(
            log.wait_for("output.write:o", writes + 10).await,
            "still writing"
        );

        handle.set_target(LightingTarget::Off).await.unwrap();
        assert_eq!(handle.snapshot().borrow().output_issue, None);
        engine.shutdown().await.unwrap();
    })
    .await;
}

fn quiet(after: u32, for_ms: u64) -> Settings {
    settings(
        FakeSourceSettings {
            name: "s".into(),
            quiet_after: Some(after),
            quiet_for_ms: for_ms,
            ..Default::default()
        },
        FakeOutputSettings {
            name: "o".into(),
            ..Default::default()
        },
    )
}

#[tokio::test]
async fn a_source_without_frames_never_takes_the_output() {
    within(async {
        let log = CallLog::new();
        let (handle, engine, _) = start(&log, quiet(0, 0), DEADLINE).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        let waiting = snapshot_until(&handle, |s| {
            s.input.is_some_and(|i| !i.receiving && i.fps == 0.0)
        })
        .await;
        assert_eq!(waiting.session, SessionState::Running);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(log.count("output.open:o"), 0, "{:?}", log.events());

        // Off leaves an untouched output untouched.
        handle.set_target(LightingTarget::Off).await.unwrap();
        assert_eq!(log.events(), ["source.open:s", "source.close:s"]);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn a_quiet_source_releases_the_output_until_frames_return() {
    within(async {
        let log = CallLog::new();
        let (handle, engine, _) = start(&log, quiet(3, 3_600), DEADLINE).await;
        let colors = handle.colors();
        handle.set_target(LightingTarget::Processed).await.unwrap();
        assert!(log.wait_for("output.write:o", 3).await);
        assert_eq!(log.count("output.open:o"), 1);
        colors_until(&colors, true).await;

        // After the grace period the device returns to its own state, and
        // the colors it no longer shows are withdrawn.
        assert!(log.wait_for("output.idle:o:Native", 1).await);
        assert!(colors.latest().is_none());
        let snapshot = handle.snapshot().borrow().clone();
        assert_eq!(snapshot.session, SessionState::Running);
        assert!(snapshot.input.is_some_and(|i| !i.receiving));
        assert!(snapshot.failure.is_none());

        // The next frame takes it again.
        assert!(log.wait_for("output.open:o", 2).await);
        assert!(log.position("output.idle:o:Native") < log.last_position("output.open:o"));
        let writes = log.count("output.write:o");
        assert!(log.wait_for("output.write:o", writes + 1).await);
        colors_until(&colors, true).await;

        handle.set_target(LightingTarget::Off).await.unwrap();
        assert!(colors.latest().is_none());
        assert_eq!(log.count("output.idle:o:Off"), 1);
        assert_eq!(log.count("output.open:o"), 2);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn stopping_while_released_leaves_the_output_in_the_stop_mode() {
    within(async {
        let log = CallLog::new();
        let (handle, engine, _) = start(&log, quiet(2, 0), DEADLINE).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        assert!(log.wait_for("output.idle:o:Native", 1).await);

        // Already native: nothing to reopen.
        handle
            .set_target(LightingTarget::DeviceNative)
            .await
            .unwrap();
        assert_eq!(log.count("output.open:o"), 1);
        assert_eq!(handle.snapshot().borrow().session, SessionState::Native);

        handle.set_target(LightingTarget::Processed).await.unwrap();
        assert!(log.wait_for("output.idle:o:Native", 2).await);
        // Released to native, stopped Off: reopened only to turn it off.
        handle.set_target(LightingTarget::Off).await.unwrap();
        assert_eq!(log.count("output.open:o"), 3);
        assert!(before(&log, "source.close:s", "output.idle:o:Off"));
        assert_eq!(log.count("output.idle:o:Off"), 1);
        engine.shutdown().await.unwrap();
    })
    .await;
}

async fn colors_until(watch: &ColorsWatch, present: bool) {
    while watch.latest().is_some() != present {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// Colors are those the LEDs show: none while off or showing the device's own
/// effects, and none published at all while nobody watches.
#[tokio::test]
async fn colors_follow_the_lights_and_only_while_watched() {
    within(async {
        let log = CallLog::new();
        let (handle, engine, _) = start(&log, named("a", "a"), DEADLINE).await;
        let colors = handle.colors();
        assert_eq!(handle.colors_viewers(), 1);
        assert_eq!(handle.preview_viewers(), 0, "colors are not a preview");
        assert!(colors.latest().is_none(), "off");

        handle.set_target(LightingTarget::Processed).await.unwrap();
        colors_until(&colors, true).await;
        let written = colors.latest().unwrap();
        assert!(!written.top.is_empty() && !written.left.is_empty());

        handle
            .set_target(LightingTarget::DeviceNative)
            .await
            .unwrap();
        assert!(colors.latest().is_none(), "effects show their own colors");

        handle.set_target(LightingTarget::Processed).await.unwrap();
        colors_until(&colors, true).await;
        handle.set_target(LightingTarget::Off).await.unwrap();
        assert!(colors.latest().is_none());
        drop(colors);
        assert_eq!(handle.colors_viewers(), 0);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn a_settings_save_after_a_failed_start_from_native_retries_and_keeps_the_failure() {
    within(async {
        let log = CallLog::new();
        let failing = settings(
            FakeSourceSettings {
                name: "a".into(),
                fail_open: true,
                ..Default::default()
            },
            FakeOutputSettings {
                name: "a".into(),
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, failing.clone(), DEADLINE).await;
        // The lights run their own effects, then screen sync is asked for and
        // the source cannot open: the session stays native, the failure set.
        handle
            .set_target(LightingTarget::DeviceNative)
            .await
            .unwrap();
        let refused = handle
            .set_target(LightingTarget::Processed)
            .await
            .unwrap_err();
        assert_eq!(
            refused.section(),
            Some(Section::Capture),
            "opening the source"
        );
        let failed = handle.snapshot().borrow().clone();
        assert_eq!(failed.target, LightingTarget::Processed);
        assert_eq!(failed.session, SessionState::Native);
        assert!(failed.failure.is_some());
        assert_eq!(log.count("source.open_failed:a"), 1);

        // A processing-only save (brightness) is the next attempt, not a
        // silent success: the failure stays while the source still fails.
        let mut brighter = failing.clone();
        brighter.processing.brightness_percent = 75;
        handle
            .replace_settings(revision(&handle), brighter)
            .await
            .unwrap();
        assert_eq!(log.count("source.open_failed:a"), 2, "retried");
        let snapshot = handle.snapshot().borrow().clone();
        assert_eq!(snapshot.target, LightingTarget::Processed);
        assert!(snapshot.failure.is_some(), "the failure is not cleared");

        // Once the source works, the next save starts screen sync.
        let mut fixed = named("a", "a");
        fixed.processing.brightness_percent = 75;
        handle
            .replace_settings(revision(&handle), fixed)
            .await
            .unwrap();
        let snapshot = handle.snapshot().borrow().clone();
        assert_eq!(snapshot.session, SessionState::Running);
        assert!(snapshot.failure.is_none());
        engine.shutdown().await.unwrap();
    })
    .await;
}

/// Wait until `input.behind` is `wanted`, giving up after `seconds`.
async fn behind_within(handle: &EngineHandle, wanted: Behind, seconds: u64) -> bool {
    tokio::time::timeout(
        Duration::from_secs(seconds),
        handle
            .snapshot()
            .wait_for(|s| s.input.is_some_and(|input| input.behind == wanted)),
    )
    .await
    .is_ok()
}

/// A picture that takes longer to turn into colours than the source takes to
/// send the next: processing is what keeps Prysm behind, after the grace.
#[tokio::test]
async fn a_slow_processing_step_is_named_after_a_few_seconds() {
    within(async {
        let log = CallLog::new();
        let large = settings(
            FakeSourceSettings {
                name: "a".into(),
                // 4K: far slower to process than the 2 ms between frames,
                // also in an optimized build.
                width: 3840,
                height: 2160,
                ..Default::default()
            },
            FakeOutputSettings {
                name: "a".into(),
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, large, DEADLINE).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        // Not after the first slow second: the grace is several windows.
        snapshot_until(&handle, |s| s.input.is_some_and(|i| i.receiving)).await;
        assert_eq!(
            handle.snapshot().borrow().input.unwrap().behind,
            Behind::None
        );
        assert!(behind_within(&handle, Behind::Processing, 8).await);
        engine.shutdown().await.unwrap();
    })
    .await;
}

/// Lights that take longer to write than the source takes to send the next.
#[tokio::test]
async fn a_slow_output_is_named_after_a_few_seconds() {
    within(async {
        let log = CallLog::new();
        let slow = settings(
            FakeSourceSettings {
                name: "a".into(),
                ..Default::default()
            },
            FakeOutputSettings {
                name: "a".into(),
                // Far longer than the source's 2 ms sleep can overrun on a
                // loaded machine, so the loop stays saturated there too.
                write_delay_ms: 200,
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, slow, DEADLINE).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        assert!(behind_within(&handle, Behind::Output, 8).await);
        engine.shutdown().await.unwrap();
    })
    .await;
}

/// A source that sends few frames (here every 60 ms, with a 20 ms write)
/// leaves the loop idle between them: Prysm keeps up, however slow each
/// frame is.
#[tokio::test]
async fn a_slow_source_is_not_falling_behind() {
    within(async {
        let log = CallLog::new();
        let sparse = settings(
            FakeSourceSettings {
                name: "a".into(),
                frame_interval_ms: 60,
                ..Default::default()
            },
            FakeOutputSettings {
                name: "a".into(),
                write_delay_ms: 20,
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, sparse, DEADLINE).await;
        handle.set_target(LightingTarget::Processed).await.unwrap();
        snapshot_until(&handle, |s| s.input.is_some_and(|i| i.receiving)).await;
        // Five windows: well past the grace, still keeping up.
        assert!(!behind_within(&handle, Behind::Output, 5).await);
        assert_eq!(
            handle.snapshot().borrow().input.unwrap().behind,
            Behind::None
        );
        engine.shutdown().await.unwrap();
    })
    .await;
}
