//! Production engine ordering with adapter and ownership gates.
use crate::common::*;
use prysm_ports::{
    LightingMode, LightingTarget, Power, SessionPhase, SessionState, SettingsStore, WaitingFor,
};
use prysm_runtime::fakes::{
    AtStop, CallLog, FakeDisplay, FakeIdle, FakeOpen, FakeOutputSettings, FakeSourceSettings,
    MemoryStore,
};
use prysm_runtime::{Engine, EngineHandle};

async fn target(handle: &EngineHandle, wanted: LightingTarget) {
    handle.set_target(wanted).await.unwrap();
}

#[tokio::test]
async fn fresh_boot_leaves_external_stream_alone_and_resumes_only_idle_screen_sync() {
    within_async(LIMIT, async {
        for (device, external, starts) in [
            (FakeDisplay::Live, true, false),
            (FakeDisplay::Off, false, false),
            (FakeDisplay::Idle, false, true),
            (FakeDisplay::Override, false, false),
        ] {
            let log = CallLog::new();
            let mut store = MemoryStore::with(settings(
                FakeSourceSettings {
                    name: "s".into(),
                    quiet_after: Some(0),
                    ..Default::default()
                },
                FakeOutputSettings {
                    name: "o".into(),
                    startup: Some(device),
                    ..Default::default()
                },
            ));
            store.save_mode(LightingMode::ScreenSync).unwrap();
            let (handle, engine) = Engine::start(registry(&log), Box::new(store.clone()), DEADLINE)
                .await
                .unwrap();
            let snapshot = snapshot_until(&handle, |snapshot| {
                snapshot.control.power.is_some()
                    && (!starts || matches!(snapshot.session, SessionState::ScreenSync))
            })
            .await;
            assert_eq!(snapshot.control.external, external);
            match device {
                FakeDisplay::Override => assert_eq!(snapshot.target, LightingTarget::Effects),
                FakeDisplay::Idle => {
                    assert_eq!(snapshot.target, LightingTarget::ScreenSync);
                    assert!(before(&log, "source.open:s", "output.intent:o:Resume"));
                    assert_eq!(handle.stats().frames_received, 0, "stable without a frame");
                }
                _ => assert_eq!(snapshot.target, LightingTarget::Off),
            }
            assert_eq!(log.count("source.open:s"), usize::from(starts));
            assert_eq!(log.count("output.idle:o:Native"), 0);
            engine.shutdown().await.unwrap();
            if device == FakeDisplay::Override {
                assert_eq!(store.load_mode().unwrap(), Some(LightingMode::Effects));
            }
        }
    })
    .await;
}

#[tokio::test]
async fn dark_waiting_request_cancels_without_sending_off_and_can_be_started_again() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(
            &log,
            settings(
                FakeSourceSettings {
                    name: "s".into(),
                    ready_on: Some("card".into()),
                    ..Default::default()
                },
                FakeOutputSettings {
                    name: "o".into(),
                    startup: Some(FakeDisplay::Off),
                    ..Default::default()
                },
            ),
            DEADLINE,
        )
        .await;
        snapshot_until(&handle, |s| s.control.power == Some(Power::Off)).await;
        target(&handle, LightingTarget::ScreenSync).await;
        snapshot_until(&handle, |s| s.control.waiting == Some(WaitingFor::Capture)).await;
        assert!(!handle.snapshot().borrow().control.busy);
        assert!(log.wait_for("source.monitor", 1).await);
        assert_eq!(log.count("source.open:s"), 0);
        handle.press_power().await.unwrap();
        assert_eq!(handle.snapshot().borrow().target, LightingTarget::Off);
        log.record("card");
        assert_eq!(log.count("source.open:s"), 0);
        assert_eq!(log.count("output.idle:o:Off"), 0);
        handle.press_power().await.unwrap();
        snapshot_until(&handle, |s| matches!(s.session, SessionState::ScreenSync)).await;
        assert!(before(&log, "source.open:s", "output.intent:o:Explicit"));
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn power_waits_for_the_output_open_and_unlocks_when_sync_exits() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(
            &log,
            settings(
                FakeSourceSettings {
                    name: "s".into(),
                    close_on: Some("release".into()),
                    ..Default::default()
                },
                FakeOutputSettings {
                    name: "o".into(),
                    open: FakeOpen::UntilStop(AtStop::Opened),
                    ..Default::default()
                },
            ),
            DEADLINE,
        )
        .await;
        target(&handle, LightingTarget::ScreenSync).await;
        assert!(log.wait_for("output.opening:o", 1).await);
        snapshot_until(&handle, |snapshot| snapshot.control.busy).await;
        handle.press_power().await.unwrap();
        assert_eq!(
            handle.snapshot().borrow().target,
            LightingTarget::ScreenSync
        );

        target(&handle, LightingTarget::Effects).await;
        assert!(log.wait_for("source.closing:s", 1).await);
        assert!(!handle.snapshot().borrow().control.busy);
        handle.press_power().await.unwrap();
        assert_eq!(handle.snapshot().borrow().target, LightingTarget::Off);
        log.record("release");
        assert!(log.wait_for("output.idle:o:Off", 1).await);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn a_blocked_output_outranks_the_current_capture_wait() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(
            &log,
            settings(
                FakeSourceSettings {
                    name: "s".into(),
                    ready_on: Some("card".into()),
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
        target(&handle, LightingTarget::ScreenSync).await;
        snapshot_until(&handle, |s| s.control.waiting == Some(WaitingFor::Capture)).await;

        let mut without_output = handle.snapshot().borrow().settings.clone();
        without_output.output = None;
        handle
            .replace_settings(revision(&handle), without_output)
            .await
            .unwrap();

        let snapshot = handle.snapshot().borrow().clone();
        assert_eq!(snapshot.target, LightingTarget::Off);
        assert_eq!(snapshot.control.waiting, None);
        assert_eq!(
            snapshot.session.failure().unwrap().to_string(),
            prysm_ports::ValidationError::new("output", "not configured").to_string()
        );
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn partial_open_failure_releases_capture_attempts_off_and_stays_off_after_fix() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(
            &log,
            settings(
                FakeSourceSettings {
                    name: "s".into(),
                    ..Default::default()
                },
                FakeOutputSettings {
                    name: "o".into(),
                    open: FakeOpen::Fails,
                    ..Default::default()
                },
            ),
            DEADLINE,
        )
        .await;
        target(&handle, LightingTarget::ScreenSync).await;
        snapshot_until(&handle, |s| {
            s.target == LightingTarget::Off && s.session.failure().is_some()
        })
        .await;
        assert!(before(&log, "output.open_failed:o", "source.close:s"));
        assert!(before(&log, "source.close:s", "output.idle:o:Off"));
        handle
            .replace_settings(revision(&handle), named("s", "new"))
            .await
            .unwrap();
        assert_eq!(handle.snapshot().borrow().target, LightingTarget::Off);
        assert_eq!(log.count("source.open:s"), 1);
        assert_eq!(log.count("output.open:new"), 0);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn reconnect_during_close_restarts_after_confirmation_or_yields_to_new_effects() {
    within_async(LIMIT, async {
        for replace in [false, true] {
            let log = CallLog::new();
            let (handle, engine, _) = start(
                &log,
                settings(
                    FakeSourceSettings {
                        name: "s".into(),
                        fail_after: Some(1),
                        reconnect_on: Some("reconnected".into()),
                        close_on: Some("release".into()),
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
            target(&handle, LightingTarget::ScreenSync).await;
            assert!(log.wait_for("source.closing:s", 1).await);
            log.record("reconnected");
            if replace {
                target(&handle, LightingTarget::Effects).await;
            }
            assert_eq!(log.count("source.open:s"), 1);
            assert_eq!(log.count("output.idle:o:Off"), 0);
            log.record("release");
            if replace {
                snapshot_until(&handle, |s| {
                    matches!(s.session, SessionState::Effects { .. })
                })
                .await;
                assert_eq!(
                    log.count("output.idle:o:Off"),
                    0,
                    "the unsent Off was retired"
                );
                assert_eq!(log.count("source.open:s"), 1);
            } else {
                assert!(log.wait_for("source.open:s", 2).await);
                assert!(log.position("source.close:s") < log.position("output.idle:o:Off"));
                assert!(log.position("output.idle:o:Off") < log.last_position("source.open:s"));
            }
            engine.shutdown().await.unwrap();
        }
    })
    .await;
}

#[tokio::test]
async fn effects_checks_current_power_after_pause_or_waiting_for_capture() {
    within_async(LIMIT, async {
        for waiting in [false, true] {
            let log = CallLog::new();
            let (handle, engine, _) = start(
                &log,
                settings(
                    FakeSourceSettings {
                        name: "s".into(),
                        ready_on: Some("card".into()),
                        ..Default::default()
                    },
                    FakeOutputSettings {
                        name: "o".into(),
                        startup: Some(FakeDisplay::Idle),
                        ..Default::default()
                    },
                ),
                DEADLINE,
            )
            .await;
            snapshot_until(&handle, |s| {
                matches!(s.session, SessionState::Effects { .. })
            })
            .await;
            assert_eq!(log.count("output.idle:o:Native"), 0);
            if waiting {
                target(&handle, LightingTarget::ScreenSync).await;
                snapshot_until(&handle, |s| s.control.waiting == Some(WaitingFor::Capture)).await;
                target(&handle, LightingTarget::Effects).await;
            } else {
                handle.pause().await.unwrap();
                assert_eq!(handle.snapshot().borrow().target, LightingTarget::Off);
                handle.press_power().await.unwrap();
            }
            snapshot_until(&handle, |s| {
                matches!(s.session, SessionState::Effects { .. })
            })
            .await;
            assert_eq!(log.count("output.idle:o:Native"), 1);
            assert_eq!(log.count("output.open:o"), 0);
            engine.shutdown().await.unwrap();
        }
    })
    .await;
}

#[tokio::test]
async fn a_retired_effects_failure_cannot_block_the_latest_request() {
    within_async(LIMIT, async {
        for change_settings in [false, true] {
            let log = CallLog::new();
            let initial = settings(
                FakeSourceSettings {
                    name: "s".into(),
                    ..Default::default()
                },
                FakeOutputSettings {
                    name: "o".into(),
                    idle: FakeIdle::FailsAfter("command settles".into()),
                    ..Default::default()
                },
            );
            let (handle, engine, _) = start(&log, initial.clone(), DEADLINE).await;
            target(&handle, LightingTarget::Effects).await;
            assert!(log.wait_for("output.idle:o:Native", 1).await);

            if change_settings {
                handle
                    .replace_settings(revision(&handle), named("s", "other"))
                    .await
                    .unwrap();
                handle
                    .replace_settings(revision(&handle), initial)
                    .await
                    .unwrap();
            } else {
                target(&handle, LightingTarget::ScreenSync).await;
            }
            assert!(handle.snapshot().borrow().control.busy);
            let target_before_press = handle.snapshot().borrow().target;
            handle.press_power().await.unwrap();
            assert_eq!(handle.snapshot().borrow().target, target_before_press);
            let old_calls = log.count("output.idle:o:Native");
            let capture_started = log.count("source.open:s");
            log.record("command settles");
            assert_eq!(old_calls, 1);
            assert_eq!(capture_started, 0);

            if change_settings {
                assert!(log.wait_for("output.idle:o:Native", 2).await);
                snapshot_until(&handle, |s| {
                    s.target == LightingTarget::Off && s.session.failure().is_some()
                })
                .await;
                engine.shutdown().await.unwrap();
            } else {
                snapshot_until(&handle, |s| matches!(s.session, SessionState::ScreenSync)).await;
                assert!(engine.shutdown().await.is_err(), "the fake rejects Off too");
            }
        }
    })
    .await;
}

#[tokio::test]
async fn a_retired_session_idle_failure_cannot_block_reselected_output_settings() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let initial = settings(
            FakeSourceSettings {
                name: "s".into(),
                ..Default::default()
            },
            FakeOutputSettings {
                name: "o".into(),
                idle: FakeIdle::FailsAfter("idle settles".into()),
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, initial.clone(), DEADLINE).await;
        target(&handle, LightingTarget::ScreenSync).await;
        snapshot_until(&handle, |s| matches!(s.session, SessionState::ScreenSync)).await;
        handle
            .replace_settings(revision(&handle), named("s", "other"))
            .await
            .unwrap();
        assert!(log.wait_for("output.idle:o:Native", 1).await);
        handle
            .replace_settings(revision(&handle), initial)
            .await
            .unwrap();
        log.record("idle settles");

        snapshot_until(&handle, |s| matches!(s.session, SessionState::ScreenSync)).await;
        assert_eq!(log.count("output.open:o"), 2);
        assert_eq!(log.count("output.open:other"), 0);
        assert!(engine.shutdown().await.is_err(), "the fake rejects Off too");
    })
    .await;
}

#[tokio::test]
async fn an_output_release_panic_prevents_reopening_after_settings_change() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(
            &log,
            settings(
                FakeSourceSettings {
                    name: "s".into(),
                    ..Default::default()
                },
                FakeOutputSettings {
                    name: "o".into(),
                    panic_idle: true,
                    ..Default::default()
                },
            ),
            DEADLINE,
        )
        .await;
        target(&handle, LightingTarget::ScreenSync).await;
        snapshot_until(&handle, |s| matches!(s.session, SessionState::ScreenSync)).await;
        handle
            .replace_settings(revision(&handle), named("s", "other"))
            .await
            .unwrap();
        snapshot_until(&handle, |s| {
            matches!(s.session, SessionState::Faulted(SessionPhase::IdlingOutput))
        })
        .await;
        assert!(before(&log, "source.close:s", "output.idle:o:Native"));
        assert_eq!(log.count("source.open:s"), 1);
        assert_eq!(log.count("output.open:other"), 0);
        assert!(matches!(
            engine.shutdown().await,
            Err(prysm_ports::Error::Faulted(SessionPhase::IdlingOutput))
        ));
    })
    .await;
}

#[tokio::test]
async fn off_timeout_is_one_shot_and_next_power_requests_the_remembered_mode() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(
            &log,
            settings(
                FakeSourceSettings {
                    name: "s".into(),
                    ..Default::default()
                },
                FakeOutputSettings {
                    name: "o".into(),
                    idle: FakeIdle::UnansweredUntil("returns".into()),
                    startup: Some(FakeDisplay::Idle),
                    ..Default::default()
                },
            ),
            DEADLINE,
        )
        .await;
        snapshot_until(&handle, |s| {
            matches!(s.session, SessionState::Effects { .. })
        })
        .await;
        handle.press_power().await.unwrap();
        snapshot_until(&handle, |s| {
            s.target == LightingTarget::Off && !s.control.busy && s.session.failure().is_some()
        })
        .await;
        assert_eq!(handle.snapshot().borrow().control.power, None);
        assert_eq!(log.count("output.idle:o:Off"), 1);
        log.record("returns");
        handle.press_power().await.unwrap();
        snapshot_until(&handle, |s| {
            matches!(s.session, SessionState::Effects { .. })
        })
        .await;
        assert_eq!(log.count("output.idle:o:Off"), 1);
        assert_eq!(log.count("output.idle:o:Native"), 1);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn observed_override_hands_off_without_idle_and_persists_effects() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, mut store) = start(
            &log,
            settings(
                FakeSourceSettings {
                    name: "s".into(),
                    ..Default::default()
                },
                FakeOutputSettings {
                    name: "o".into(),
                    observed_override: true,
                    overridden_after: Some(1),
                    ..Default::default()
                },
            ),
            DEADLINE,
        )
        .await;
        target(&handle, LightingTarget::ScreenSync).await;
        snapshot_until(&handle, |s| {
            matches!(s.session, SessionState::Effects { .. })
        })
        .await;
        engine.shutdown().await.unwrap();
        assert_eq!(log.count("source.close:s"), 1);
        assert_eq!(log.count("output.idle:o:Off"), 0);
        assert_eq!(log.count("output.idle:o:Native"), 0);
        assert_eq!(store.load_mode().unwrap(), Some(LightingMode::Effects));
    })
    .await;
}

#[tokio::test]
async fn unsafe_release_attempts_pending_off_once_unless_effects_supersedes_it() {
    within_async(LIMIT, async {
        for superseded in [false, true] {
            let log = CallLog::new();
            let (handle, engine, _) = start(
                &log,
                settings(
                    FakeSourceSettings {
                        name: "s".into(),
                        fail_close: true,
                        close_on: Some("release".into()),
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
            target(&handle, LightingTarget::ScreenSync).await;
            snapshot_until(&handle, |s| matches!(s.session, SessionState::ScreenSync)).await;
            target(&handle, LightingTarget::Off).await;
            assert!(log.wait_for("source.closing:s", 1).await);
            if superseded {
                target(&handle, LightingTarget::Effects).await;
            }
            log.record("release");
            snapshot_until(&handle, |s| {
                matches!(
                    s.session,
                    SessionState::Faulted(SessionPhase::SourceUnreleased)
                )
            })
            .await;
            assert!(handle.set_target(LightingTarget::ScreenSync).await.is_err());
            assert_eq!(log.count("source.open:s"), 1);
            assert_eq!(log.count("output.idle:o:Off"), usize::from(!superseded));
            assert_eq!(log.count("output.idle:o:Native"), 0);
            assert!(engine.shutdown().await.is_err());
        }
    })
    .await;
}

#[tokio::test]
async fn worker_crash_after_both_opens_restarts_once_and_an_initialization_crash_stops() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(
            &log,
            settings(
                FakeSourceSettings {
                    name: "s".into(),
                    crash_then_fail_init: true,
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
        target(&handle, LightingTarget::ScreenSync).await;
        snapshot_until(&handle, |s| {
            s.target == LightingTarget::Off && s.session.failure().is_some()
        })
        .await;
        assert_eq!(log.count("source.open:s"), 1);
        assert_eq!(log.count("source.close:s"), 1);
        assert_eq!(log.count("output.open:o"), 1);
        assert!(before(&log, "output.idle:o:Off", "source.crashed_open:s"));
        assert_eq!(log.count("source.crashed_open:s"), 1);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn manual_recheck_during_startup_updates_facts_without_selecting_a_mode() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let mut store = MemoryStore::with(settings(
            FakeSourceSettings {
                name: "s".into(),
                ..Default::default()
            },
            FakeOutputSettings {
                name: "o".into(),
                startup: Some(FakeDisplay::Idle),
                unreachable_until: Some("output online".into()),
                ..Default::default()
            },
        ));
        store.save_mode(LightingMode::ScreenSync).unwrap();
        let (handle, engine) = Engine::start(registry(&log), Box::new(store), DEADLINE)
            .await
            .unwrap();
        assert!(log.wait_for("output.monitor", 1).await);
        handle.recheck().await.unwrap();
        log.record("output online");
        handle.recheck().await.unwrap();
        snapshot_until(&handle, |s| s.control.power == Some(Power::On)).await;
        assert_eq!(handle.snapshot().borrow().target, LightingTarget::Off);
        assert_eq!(log.count("source.open:s"), 0);
        assert_eq!(log.count("output.open:o"), 0);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn supervised_replacement_can_resume_an_authorized_stream_without_takeover() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(
            &log,
            settings(
                FakeSourceSettings {
                    name: "s".into(),
                    ..Default::default()
                },
                FakeOutputSettings {
                    name: "o".into(),
                    startup: Some(FakeDisplay::Live),
                    ..Default::default()
                },
            ),
            DEADLINE,
        )
        .await;
        snapshot_until(&handle, |s| s.control.external).await;
        assert_eq!(log.count("source.open:s"), 0);
        handle.resume_screen_sync().await.unwrap();
        snapshot_until(&handle, |s| matches!(s.session, SessionState::ScreenSync)).await;
        assert_eq!(log.count("output.intent:o:Resume"), 1);
        assert_eq!(log.count("output.intent:o:Explicit"), 0);
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn supervised_replacement_honors_an_override_seen_during_boot() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, mut store) = start(
            &log,
            settings(
                FakeSourceSettings {
                    name: "s".into(),
                    ..Default::default()
                },
                FakeOutputSettings {
                    name: "o".into(),
                    startup: Some(FakeDisplay::Override),
                    ..Default::default()
                },
            ),
            DEADLINE,
        )
        .await;
        snapshot_until(&handle, |s| s.target == LightingTarget::Effects).await;
        handle.resume_screen_sync().await.unwrap();
        assert!(log.wait_for("output.intent:o:Resume", 1).await);
        snapshot_until(&handle, |s| {
            matches!(s.session, SessionState::Effects { .. })
        })
        .await;
        engine.shutdown().await.unwrap();
        assert_eq!(store.load_mode().unwrap(), Some(LightingMode::Effects));
        assert_eq!(log.count("source.close:s"), 1);
        assert_eq!(log.count("output.intent:o:Explicit"), 0);
        assert_eq!(log.count("output.open:o"), 0);
        assert_eq!(log.count("output.idle:o:Off"), 0);
        assert_eq!(log.count("output.idle:o:Native"), 0);
    })
    .await;
}
