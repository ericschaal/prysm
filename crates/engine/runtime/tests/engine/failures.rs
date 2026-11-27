//! Adapter and surface failures and panics: what is reported and what still runs.
use crate::common::*;
use prysm_ports::{Error, LightingTarget, Processing, Section, SessionState};
use prysm_runtime::fakes::{
    CallLog, FakeOutputSettings, FakeOutputs, FakeSourceSettings, FakeSources, MemoryStore,
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

        let snapshot = snapshot_until(&handle, |s| {
            s.target == LightingTarget::Off && s.session.failure().is_some()
        })
        .await;
        assert!(
            matches!(snapshot.session, SessionState::Stopped { .. }),
            "{}",
            snapshot.session
        );
        assert_eq!(snapshot.target, LightingTarget::Off);
        assert_eq!(log.count("output.write:a"), 3);
        assert!(before(&log, "source.close:a", "output.idle:a:Off"));
        assert_eq!(
            snapshot.session.failure().and_then(Error::section),
            Some(Section::Capture)
        );
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

        let snapshot = snapshot_until(&handle, |s| {
            s.target == LightingTarget::Off && s.session.failure().is_some()
        })
        .await;
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
