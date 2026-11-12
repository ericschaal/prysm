use prysm_ports::{
    AdapterSettings, Error, LightingTarget, Processing, Registry, Revision, SessionPhase,
    SessionState, Settings, Snapshot,
};
use prysm_runtime::fakes::{
    CallLog, FakeOutputSettings, FakeOutputs, FakeSourceSettings, FakeSources, MemoryStore,
};
use prysm_runtime::{Engine, EngineHandle, Prysm, Shutdown};
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
        handle.set_target(LightingTarget::Processed).await.unwrap();
        assert!(log.wait_for("output.write:a", 3).await);
        assert!(handle.preview().peek().is_some());

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
        let revision = handle
            .replace_settings(Revision(0), changed.clone())
            .await
            .unwrap();
        assert_eq!(revision, Revision(1));
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

        handle
            .replace_settings(Revision(0), named("b", "a"))
            .await
            .unwrap();
        assert!(before(&log, "source.close:a", "output.idle:a:Off"));
        assert!(before(&log, "output.idle:a:Off", "source.open:b"));
        assert_eq!(log.count("output.open:a"), 2);
        assert_eq!(handle.snapshot().borrow().session, SessionState::Running);

        handle
            .replace_settings(Revision(1), named("b", "c"))
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
            .replace_settings(Revision(0), named("b", "b"))
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

        let invalid = settings(
            FakeSourceSettings {
                invalid: true,
                ..Default::default()
            },
            FakeOutputSettings::default(),
        );
        let error = handle
            .replace_settings(Revision(0), invalid)
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Invalid(ref e) if e.field == "capture.invalid"));

        let mut unknown = named("a", "a");
        unknown.output = Some(AdapterSettings::new("nope", &FakeOutputSettings::default()));
        let error = handle
            .replace_settings(Revision(0), unknown)
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Invalid(ref e) if e.field == "output.kind"));

        let mut out_of_range = named("a", "a");
        out_of_range.processing.brightness_percent = 101;
        assert!(
            handle
                .replace_settings(Revision(0), out_of_range)
                .await
                .is_err()
        );

        let error = handle
            .replace_settings(Revision(7), named("b", "b"))
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            Error::StaleRevision {
                expected: Revision(7),
                current: Revision(0)
            }
        ));

        assert_eq!(store.saved(), Some(initial));
        assert_eq!(handle.snapshot().borrow().revision, Revision(0));
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn apply_failure_keeps_settings_saved_and_reports_it() {
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
        let revision = handle
            .replace_settings(Revision(0), broken.clone())
            .await
            .unwrap();
        assert_eq!(revision, Revision(1));
        assert_eq!(store.saved(), Some(broken));

        let snapshot = handle.snapshot().borrow().clone();
        assert!(snapshot.failure.is_some());
        assert_eq!(snapshot.session, SessionState::Stopped);
        assert_eq!(snapshot.revision, Revision(1));
        // The open failure released the newly opened source.
        assert_eq!(log.count("source.close:a"), 2);
        assert_eq!(log.count("output.idle:a:Off"), 1);
        engine.shutdown().await.unwrap();
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
            handle.replace_settings(Revision(0), named("b", "b")).await,
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
