//! Adapter and surface failures and panics: what is reported and what still runs.
use crate::common::*;
use prysm_ports::{Error, LightingTarget, Processing, Section, SessionPhase, SessionState};
use prysm_runtime::fakes::{
    CallLog, FakeIdle, FakeOutputSettings, FakeOutputs, FakeSourceSettings, FakeSources,
    MemoryStore,
};
use prysm_runtime::{EngineHandle, Prysm, Shutdown};
use std::time::Duration;

#[tokio::test]
async fn output_idles_when_the_source_fails() {
    within_async(LIMIT, async {
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
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();

        let snapshot = snapshot_until(&handle, |s| s.session.failure().is_some()).await;
        assert!(
            matches!(snapshot.session, SessionState::Stopped { .. }),
            "{}",
            snapshot.session
        );
        assert_eq!(snapshot.target, LightingTarget::ScreenSync);
        assert_eq!(log.count("output.write:a"), 3);
        assert!(before(&log, "source.close:a", "output.idle:a:Off"));
        // The failure says it was the capture's.
        assert_eq!(
            snapshot.session.failure().and_then(Error::section),
            Some(Section::Capture)
        );
        // A failed source was released, so any later target is accepted and
        // the source opens again.
        handle.set_target(LightingTarget::Off).await.unwrap();
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert!(log.wait_for("source.open:a", 2).await);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn output_idles_when_a_write_fails() {
    within_async(LIMIT, async {
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
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();

        let snapshot = snapshot_until(&handle, |s| s.session.failure().is_some()).await;
        assert!(matches!(
            snapshot.session.failure(),
            Some(Error::Adapter { .. })
        ));
        assert_eq!(
            snapshot.session.failure().and_then(Error::section),
            Some(Section::Output),
            "the lights' failure"
        );
        assert_eq!(log.count("source.close:a"), 1);
        assert_eq!(log.count("output.idle:a:Off"), 1);
        // Stopped with the lights' failure, whatever the target was: `Off`
        // switches them off again, and that clears it.
        handle.set_target(LightingTarget::Off).await.unwrap();
        assert_eq!(log.count("output.idle:a:Off"), 2, "idled off again");
        let snapshot = handle.snapshot().borrow().clone();
        assert!(
            snapshot.session.failure().is_none(),
            "{:?}",
            snapshot.session
        );
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn failing_surface_does_not_stop_lighting() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let controller = {
            let log = log.clone();
            move |engine: EngineHandle, _shutdown: Shutdown| async move {
                engine.set_target(LightingTarget::ScreenSync).await?;
                engine
                    .snapshot()
                    .wait_for(|s| !s.surface_failures.is_empty())
                    .await
                    .map_err(|_| Error::Closed)?;
                let writes = log.count("output.write:a");
                assert!(log.wait_for("output.write:a", writes + 3).await);
                assert!(matches!(
                    engine.snapshot().borrow().session,
                    SessionState::ScreenSync
                ));
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

/// A surface that fails while the mailbox is full is still reported, and
/// shutdown is not held up by the report.
#[tokio::test]
async fn a_surface_failure_while_the_mailbox_is_full_is_reported() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (full, filled) = tokio::sync::oneshot::channel();
        let flooding = {
            let log = log.clone();
            move |engine: EngineHandle, _shutdown: Shutdown| async move {
                // A save the store holds for a second keeps the engine busy
                // while tried processing fills the mailbox behind it.
                let saving = engine.clone();
                let revision = revision(&engine);
                tokio::spawn(
                    async move { saving.replace_settings(revision, named("a", "a")).await },
                );
                assert!(log.wait_for("store.saving", 1).await);
                for _ in 0..24 {
                    let trying = engine.clone();
                    tokio::spawn(async move { trying.try_processing(Processing::default()).await });
                }
                for _ in 0..10 {
                    tokio::task::yield_now().await;
                }
                let _ = full.send(engine.snapshot());
                Err(Error::other("surface broke"))
            }
        };
        let store = SlowStore {
            saved: MemoryStore::with(named("a", "a")),
            log: log.clone(),
        };
        let (snapshots, snapshot) = tokio::sync::oneshot::channel();
        Prysm::builder(store)
            .source(FakeSources(log.clone()))
            .output(FakeOutputs(log.clone()))
            .surface(flooding)
            .stop_deadline(STUCK * 3)
            .run(async {
                let snapshot = filled.await.unwrap();
                // Shutdown is asked for while the report waits for room.
                tokio::time::sleep(Duration::from_millis(20)).await;
                let _ = snapshots.send(snapshot);
            })
            .await
            .unwrap();

        let snapshot = snapshot.await.unwrap();
        let failures = snapshot.borrow().surface_failures.clone();
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert_eq!(failures[0].to_string(), "surface broke");
    })
    .await;
}

#[tokio::test]
async fn stop_with_a_failing_idle_still_closes_the_source_and_reports_it() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let unreachable = settings(
            FakeSourceSettings {
                name: "a".into(),
                ..Default::default()
            },
            FakeOutputSettings {
                name: "a".into(),
                idle: FakeIdle::Fails,
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, unreachable, DEADLINE).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert!(log.wait_for("output.write:a", 1).await);

        let error = handle.set_target(LightingTarget::Off).await.unwrap_err();
        assert_eq!(error.to_string(), "fake output cannot idle");
        assert!(before(&log, "source.close:a", "output.idle:a:Off"));
        let snapshot = handle.snapshot().borrow().clone();
        assert_eq!(snapshot.target, LightingTarget::Off);
        assert!(
            matches!(snapshot.session, SessionState::Stopped { .. }),
            "{}",
            snapshot.session
        );
        assert_eq!(
            snapshot
                .session
                .failure()
                .map(|failure| failure.to_string())
                .as_deref(),
            Some("fake output cannot idle")
        );

        // Not faulted: lighting can start again, and shutdown reports the idle failure.
        let opened = log.count("output.open:a");
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert_eq!(log.count("source.open:a"), 2);
        assert!(log.wait_for("output.open:a", opened + 1).await);
        let shutdown = engine.shutdown().await.unwrap_err();
        assert_eq!(shutdown.to_string(), "fake output cannot idle");
        assert_eq!(log.count("source.close:a"), 2);
    })
    .await;
}

#[tokio::test]
async fn a_source_panicking_while_closing_still_idles_the_output_and_faults_the_engine() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let panicking = settings(
            FakeSourceSettings {
                name: "a".into(),
                panic_close: true,
                ..Default::default()
            },
            FakeOutputSettings {
                name: "a".into(),
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, panicking, DEADLINE).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert!(log.wait_for("output.write:a", 1).await);

        let error = handle.set_target(LightingTarget::Off).await.unwrap_err();
        assert!(matches!(error, Error::Panicked(_)), "{error:?}");
        assert_eq!(log.count("output.idle:a:Off"), 1);
        // The source's release is unconfirmed: nothing may reopen it.
        assert!(matches!(
            handle.snapshot().borrow().session,
            SessionState::Faulted(SessionPhase::SourceUnreleased)
        ));
        assert!(engine.shutdown().await.is_err());
    })
    .await;
}

#[tokio::test]
async fn an_output_panicking_while_idling_is_reported_and_lighting_can_start_again() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let panicking = settings(
            FakeSourceSettings {
                name: "a".into(),
                ..Default::default()
            },
            FakeOutputSettings {
                name: "a".into(),
                panic_idle: true,
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, panicking, DEADLINE).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert!(log.wait_for("output.write:a", 1).await);

        let error = handle.set_target(LightingTarget::Off).await.unwrap_err();
        assert!(matches!(error, Error::Panicked(_)), "{error:?}");
        assert_eq!(log.count("source.close:a"), 1);
        assert!(matches!(
            handle.snapshot().borrow().session,
            SessionState::Stopped { .. }
        ));
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert_eq!(log.count("source.open:a"), 2);
        assert!(log.wait_for("output.open:a", 2).await);
        assert!(engine.shutdown().await.is_err());
    })
    .await;
}

/// An output that panics while being put in its own effects did not reach
/// them: the state does not claim it did.
#[tokio::test]
async fn an_effects_idle_that_panics_leaves_the_session_stopped() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(&log, named("s", "a"), DEADLINE).await;
        handle.set_target(LightingTarget::Effects).await.unwrap();
        assert!(matches!(
            handle.snapshot().borrow().session,
            SessionState::Effects { .. }
        ));

        let panicking = settings(
            FakeSourceSettings {
                name: "s".into(),
                ..Default::default()
            },
            FakeOutputSettings {
                name: "b".into(),
                panic_idle: true,
                ..Default::default()
            },
        );
        handle
            .replace_settings(revision(&handle), panicking)
            .await
            .unwrap();
        assert_eq!(log.count("output.idle:b:Native"), 1);
        let snapshot = handle.snapshot().borrow().clone();
        assert!(
            matches!(snapshot.session, SessionState::Stopped { .. }),
            "{}",
            snapshot.session
        );
        assert!(matches!(
            snapshot.session.failure(),
            Some(Error::Panicked(_))
        ));
        engine.shutdown().await.unwrap();
    })
    .await;
}
