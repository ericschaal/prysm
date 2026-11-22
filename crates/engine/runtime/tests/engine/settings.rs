//! Settings changes and the restarts they cause, revisions and validation.
use crate::common::*;
use prysm_ports::{
    AdapterSettings, Error, LightingTarget, Revision, Section, SessionState, Snapshot,
};
use prysm_runtime::Engine;
use prysm_runtime::fakes::{CallLog, FakeOutputSettings, FakeSourceSettings, MemoryStore};

#[tokio::test]
async fn processing_change_keeps_source_and_output_open() {
    within_async(LIMIT, async {
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
    within_async(LIMIT, async {
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
    within_async(LIMIT, async {
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
    within_async(LIMIT, async {
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
    within_async(LIMIT, async {
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
async fn each_engine_starts_at_its_own_revision_so_old_revisions_are_stale() {
    within_async(LIMIT, async {
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

#[tokio::test]
async fn output_change_while_off_neither_opens_nor_powers_the_new_output() {
    within_async(LIMIT, async {
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
async fn concurrent_replacements_of_one_revision_admit_exactly_one() {
    within_async(LIMIT, async {
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
    within_async(LIMIT, async {
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
async fn a_settings_save_after_a_failed_start_from_native_retries_and_keeps_the_failure() {
    within_async(LIMIT, async {
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

#[tokio::test]
async fn a_restart_starts_the_new_session_even_when_the_old_output_cannot_idle() {
    within_async(LIMIT, async {
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

        // The lights that no longer answer are replaced.
        handle
            .replace_settings(revision(&handle), named("a", "b"))
            .await
            .unwrap();
        assert_eq!(log.count("output.idle:a:Off"), 1);
        assert!(log.wait_for("output.write:b", 1).await);
        let snapshot = handle.snapshot().borrow().clone();
        assert_eq!(snapshot.session, SessionState::Running);
        assert_eq!(
            snapshot
                .failure
                .map(|failure| failure.to_string())
                .as_deref(),
            Some("fake output cannot idle"),
            "the old output's failure is still reported"
        );
        engine.shutdown().await.unwrap();
        assert_eq!(log.count("output.idle:b:Off"), 1);
    })
    .await;
}

#[tokio::test]
async fn a_restart_that_cannot_start_reports_the_start_failure() {
    within_async(LIMIT, async {
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

        let broken = settings(
            FakeSourceSettings {
                name: "b".into(),
                fail_open: true,
                ..Default::default()
            },
            FakeOutputSettings {
                name: "a".into(),
                ..Default::default()
            },
        );
        handle
            .replace_settings(revision(&handle), broken)
            .await
            .unwrap();
        assert_eq!(log.count("source.open_failed:b"), 1, "the start was tried");
        let snapshot = handle.snapshot().borrow().clone();
        assert_eq!(snapshot.session, SessionState::Stopped);
        assert_eq!(
            snapshot.failure.as_ref().and_then(Error::section),
            Some(Section::Capture),
            "the start's failure, not the old idle's"
        );
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn a_settings_save_after_a_failed_native_idle_retries_the_idle() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let unreachable = settings(
            FakeSourceSettings::default(),
            FakeOutputSettings {
                name: "a".into(),
                fail_idle: true,
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, unreachable.clone(), DEADLINE).await;
        assert!(
            handle
                .set_target(LightingTarget::DeviceNative)
                .await
                .is_err()
        );
        let failed = handle.snapshot().borrow().clone();
        assert_eq!(failed.target, LightingTarget::DeviceNative);
        assert_eq!(failed.session, SessionState::Stopped);
        assert!(failed.failure.is_some());

        // A processing-only save retries, and the failure stays.
        let mut brighter = unreachable;
        brighter.processing.brightness_percent = 75;
        handle
            .replace_settings(revision(&handle), brighter)
            .await
            .unwrap();
        assert_eq!(log.count("output.idle:a:Native"), 2, "retried");
        assert!(handle.snapshot().borrow().failure.is_some());

        // Corrected output settings put the lights in their own effects.
        handle
            .replace_settings(revision(&handle), named("", "b"))
            .await
            .unwrap();
        assert_eq!(log.count("output.idle:b:Native"), 1);
        let snapshot = handle.snapshot().borrow().clone();
        assert_eq!(snapshot.session, SessionState::Native);
        assert!(snapshot.failure.is_none());
        engine.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn invalid_loaded_settings_stay_reported_after_a_successful_command() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let invalid = settings(
            FakeSourceSettings {
                invalid: true,
                ..Default::default()
            },
            FakeOutputSettings::default(),
        );
        let (handle, engine, _) = start(&log, invalid, DEADLINE).await;
        let invalid_capture = |s: &Snapshot| {
            matches!(&s.failure, Some(Error::Invalid(e)) if e.field == "capture.invalid")
        };
        assert!(invalid_capture(&handle.snapshot().borrow()));

        // The output alone is valid, so the lights take their own effects.
        handle
            .set_target(LightingTarget::DeviceNative)
            .await
            .unwrap();
        assert_eq!(handle.snapshot().borrow().session, SessionState::Native);
        assert!(invalid_capture(&handle.snapshot().borrow()));

        handle
            .replace_settings(revision(&handle), named("a", "a"))
            .await
            .unwrap();
        assert!(handle.snapshot().borrow().failure.is_none());
        engine.shutdown().await.unwrap();
    })
    .await;
}
