//! Stopping and shutting down: the stop order, the stop deadline and the shutdown grace.
use crate::common::*;
use prysm_ports::{Error, LightingTarget, Registry, SessionPhase, SessionState};
use prysm_runtime::Engine;
use prysm_runtime::fakes::{
    CallLog, FakeOutputSettings, FakeOutputs, FakeSourceSettings, FakeSources, MemoryStore,
};
use std::time::Duration;

#[tokio::test]
async fn stop_writes_then_closes_source_then_idles_output_then_joins() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(&log, named("a", "a"), DEADLINE).await;
        let preview = handle.preview();
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert!(log.wait_for("output.write:a", 3).await);
        assert!(preview.latest().is_some());

        handle.set_target(LightingTarget::Off).await.unwrap();
        log.record("joined");

        assert!(before(&log, "output.write:a", "source.close:a"));
        assert!(before(&log, "source.close:a", "output.idle:a:Off"));
        assert!(before(&log, "output.idle:a:Off", "joined"));
        let snapshot = handle.snapshot().borrow().clone();
        assert!(
            matches!(snapshot.session, SessionState::Stopped { .. }),
            "{}",
            snapshot.session
        );
        assert!(snapshot.session.failure().is_none());
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn missed_stop_deadline_faults_and_only_shutdown_is_accepted() {
    within_async(LIMIT, async {
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
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert!(log.wait_for("output.write:a", 1).await);

        let error = handle.set_target(LightingTarget::Off).await.unwrap_err();
        assert!(matches!(error, Error::Faulted(SessionPhase::ClosingSource)));
        assert!(matches!(
            handle.snapshot().borrow().session,
            SessionState::Faulted(SessionPhase::ClosingSource)
        ));
        assert!(matches!(
            handle.set_target(LightingTarget::ScreenSync).await,
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
async fn shutdown_abandons_a_command_stuck_in_an_adapter() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let stuck = settings(
            FakeSourceSettings {
                name: "a".into(),
                open_delay_ms: u64::try_from(STUCK.as_millis()).unwrap(),
                ..Default::default()
            },
            FakeOutputSettings {
                name: "a".into(),
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, stuck, STUCK_DEADLINE).await;
        let starting = tokio::spawn({
            let handle = handle.clone();
            async move { handle.set_target(LightingTarget::ScreenSync).await }
        });
        assert!(log.wait_for("source.opening:a", 1).await);

        let shutdown = shutdown_before_stuck_call_returns(engine).await;
        assert!(
            matches!(shutdown, Err(Error::Adapter { .. })),
            "{shutdown:?}"
        );
        let started = starting.await.unwrap();
        assert!(matches!(started, Err(Error::Adapter { .. })), "{started:?}");
    })
    .await;
}

#[tokio::test]
async fn stuck_calls_in_sequence_share_one_shutdown_grace() {
    within_async(LIMIT, async {
        const GRACE: Duration = Duration::from_millis(300);
        let log = CallLog::new();
        let opens_stuck = settings(
            FakeSourceSettings {
                name: "a".into(),
                open_delay_ms: u64::try_from(STUCK.as_millis()).unwrap(),
                ..Default::default()
            },
            FakeOutputSettings {
                name: "a".into(),
                ..Default::default()
            },
        );
        let store = SlowStore {
            saved: MemoryStore::with(opens_stuck.clone()),
            log: log.clone(),
        };
        let (handle, engine) = Engine::start(registry(&log), Box::new(store), STUCK_DEADLINE)
            .await
            .unwrap();
        let engine = engine.shutdown_grace(GRACE);
        let saving = tokio::spawn({
            let handle = handle.clone();
            let revision = revision(&handle);
            async move { handle.replace_settings(revision, opens_stuck).await }
        });
        assert!(log.wait_for("store.saving", 1).await);
        // Queued behind the stuck save; its open would block too.
        let starting = tokio::spawn({
            let handle = handle.clone();
            async move { handle.set_target(LightingTarget::ScreenSync).await }
        });
        tokio::task::yield_now().await;

        let asked = tokio::time::Instant::now();
        let shutdown = shutdown_before_stuck_call_returns(engine).await;
        // A grace for each stuck call would take at least two graces.
        assert!(asked.elapsed() < GRACE * 2, "{:?}", asked.elapsed());
        assert!(
            matches!(shutdown, Err(Error::Adapter { .. })),
            "{shutdown:?}"
        );
        assert!(matches!(saving.await.unwrap(), Err(Error::Adapter { .. })));
        // Still queued when shutdown was asked for: refused, never started.
        assert!(matches!(starting.await.unwrap(), Err(Error::Closed)));
        assert_eq!(log.count("source.opening:a"), 0, "the open started");
    })
    .await;
}

#[tokio::test]
async fn shutdown_abandons_a_stuck_save_and_still_stops_the_session_in_order() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let store = SlowStore {
            saved: MemoryStore::with(named("a", "a")),
            log: log.clone(),
        };
        let (handle, engine) = Engine::start(registry(&log), Box::new(store), STUCK_DEADLINE)
            .await
            .unwrap();
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert!(log.wait_for("output.write:a", 1).await);
        let saving = tokio::spawn({
            let handle = handle.clone();
            let revision = revision(&handle);
            async move { handle.replace_settings(revision, named("a", "a")).await }
        });
        assert!(log.wait_for("store.saving", 1).await);

        let shutdown = shutdown_before_stuck_call_returns(engine).await;
        assert!(
            matches!(shutdown, Err(Error::Adapter { .. })),
            "{shutdown:?}"
        );
        let saved = saving.await.unwrap();
        assert!(matches!(saved, Err(Error::Adapter { .. })), "{saved:?}");
        assert!(before(&log, "source.close:a", "output.idle:a:Off"));
        assert_eq!(log.count("output.idle:a:Off"), 1);
    })
    .await;
}

#[tokio::test]
async fn unconfirmed_source_release_faults_the_engine() {
    within_async(LIMIT, async {
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
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert!(log.wait_for("output.write:o", 1).await);
        assert!(handle.set_target(LightingTarget::Off).await.is_err());
        // The output was still idled, and nothing may reopen the device.
        assert_eq!(log.count("output.idle:o:Off"), 1);
        assert!(matches!(
            handle.snapshot().borrow().session,
            SessionState::Faulted(SessionPhase::SourceUnreleased)
        ));
        assert!(matches!(
            handle.set_target(LightingTarget::ScreenSync).await,
            Err(Error::Faulted(SessionPhase::SourceUnreleased))
        ));
        assert!(engine.shutdown().await.is_err());
    })
    .await;
}

/// A request still queued when shutdown is asked for is refused: it would
/// only start what shutdown stops.
#[tokio::test]
async fn shutdown_refuses_the_requests_queued_behind_it() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let store = SlowStore {
            saved: MemoryStore::with(named("a", "a")),
            log: log.clone(),
        };
        // Long enough that the slow save finishes instead of being abandoned.
        let (handle, engine) = Engine::start(registry(&log), Box::new(store), STUCK * 3)
            .await
            .unwrap();
        let saving = tokio::spawn({
            let handle = handle.clone();
            let revision = revision(&handle);
            async move { handle.replace_settings(revision, named("a", "a")).await }
        });
        assert!(log.wait_for("store.saving", 1).await);
        let starting = tokio::spawn({
            let handle = handle.clone();
            async move { handle.set_target(LightingTarget::ScreenSync).await }
        });
        tokio::task::yield_now().await;

        engine.shutdown().await.unwrap();
        assert!(saving.await.unwrap().is_ok(), "the save in progress ends");
        assert!(matches!(starting.await.unwrap(), Err(Error::Closed)));
        assert_eq!(log.count("source.open:a"), 0, "{:?}", log.events());
    })
    .await;
}
