//! Observations are scoped to the current request.
use crate::common::*;
use prysm_ports::{AdapterSettings, LightingTarget, SessionState, WaitingFor};
use prysm_runtime::fakes::{CallLog, FakeOutputSettings, FakeSourceSettings};

#[tokio::test]
async fn effects_wait_only_for_output_and_continue_after_it_recovers() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(
            &log,
            settings(
                FakeSourceSettings {
                    name: "s".into(),
                    ready_on: Some("never".into()),
                    ..Default::default()
                },
                FakeOutputSettings {
                    name: "o".into(),
                    unreachable_until: Some("online".into()),
                    ..Default::default()
                },
            ),
            DEADLINE,
        )
        .await;
        handle.set_target(LightingTarget::Effects).await.unwrap();
        snapshot_until(&handle, |s| s.control.waiting == Some(WaitingFor::Output)).await;
        assert!(log.wait_for("output.monitor", 1).await);
        assert_eq!(log.count("source.monitor"), 0);
        let mut changed = handle.snapshot().borrow().settings.clone();
        changed.capture = Some(AdapterSettings::new(
            "fake",
            &FakeSourceSettings {
                name: "new source".into(),
                ready_on: Some("also never".into()),
                ..Default::default()
            },
        ));
        handle
            .replace_settings(revision(&handle), changed)
            .await
            .unwrap();
        log.record("online");
        snapshot_until(&handle, |s| {
            matches!(s.session, SessionState::Effects { .. })
        })
        .await;
        assert_eq!(log.count("output.idle:o:Native"), 1);
        assert_eq!(log.count("source.open:s"), 0);
        assert_eq!(log.count("source.check"), 0);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn fresh_power_rechecks_blocked_capture_and_never_opens_on_known_red() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(
            &log,
            settings(
                FakeSourceSettings {
                    name: "s".into(),
                    ready_on: Some("permission granted".into()),
                    manual_recheck: true,
                    ..Default::default()
                },
                FakeOutputSettings {
                    name: "o".into(),
                    ..Default::default()
                },
            ),
            DEADLINE,
        )
        .await;
        for _ in 0..2 {
            handle.set_target(LightingTarget::ScreenSync).await.unwrap();
            snapshot_until(&handle, |s| {
                s.target == LightingTarget::Off && s.session.failure().is_some()
            })
            .await;
            assert_eq!(log.count("source.open:s"), 0);
        }
        log.record("permission granted");
        assert_eq!(log.count("source.open:s"), 0);
        handle.press_power().await.unwrap();
        snapshot_until(&handle, |s| matches!(s.session, SessionState::ScreenSync)).await;
        assert_eq!(log.count("source.open:s"), 1);
        engine.shutdown().await.unwrap();
    })
    .await;
}
