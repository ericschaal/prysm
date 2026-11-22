//! A session that ends or fails on its own, alone and meeting a command.
use crate::common::*;
use prysm_ports::{LightingTarget, SessionState, Settings};
use prysm_runtime::fakes::{CallLog, FakeOutputSettings, FakeSourceSettings};

#[tokio::test]
async fn an_ended_source_stops_cleanly_turns_lighting_off_and_idles_the_output() {
    within_async(LIMIT, async {
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
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();

        let snapshot = snapshot_until(&handle, |s| {
            matches!(s.session, SessionState::Stopped { .. })
        })
        .await;
        assert!(
            snapshot.session.failure().is_none(),
            "{:?}",
            snapshot.session.failure()
        );
        assert_eq!(snapshot.target, LightingTarget::Off);
        assert!(snapshot.input.is_none());
        assert_eq!(log.count("output.write:a"), 3);
        assert!(before(&log, "output.write:a", "source.close:a"));
        assert!(before(&log, "source.close:a", "output.idle:a:Off"));

        // Off: a settings change does not replay the source. A start would
        // have opened it before the save returned.
        let current = revision(&handle);
        let mut changed = handle.snapshot().borrow().settings.clone();
        changed.processing.brightness_percent =
            changed.processing.brightness_percent.saturating_sub(1);
        // The engine replies after it has reconciled the session, so a
        // replayed source would already be open.
        handle.replace_settings(current, changed).await.unwrap();
        assert_eq!(log.count("source.open:a"), 1);
        assert!(matches!(
            handle.snapshot().borrow().session,
            SessionState::Stopped { .. }
        ));

        // Asked again, it plays again.
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert!(log.wait_for("source.open:a", 2).await);
        snapshot_until(&handle, |s| s.target == LightingTarget::Off).await;
        engine.shutdown().await.unwrap();
    })
    .await;
}

/// The session ended on its own and was being left `Off` when the lights were
/// asked for their own effects: they get them, and the state says so.
#[tokio::test]
async fn effects_asked_while_a_session_ends_on_its_own_still_come_on() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let ending = settings(
            FakeSourceSettings {
                name: "s".into(),
                end_after: Some(2),
                ..Default::default()
            },
            FakeOutputSettings {
                name: "o".into(),
                idle_delay_ms: 500,
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, ending, DEADLINE).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert!(log.wait_for("output.idle:o:Off", 1).await);

        handle.set_target(LightingTarget::Effects).await.unwrap();
        assert!(before(&log, "output.idle:o:Off", "output.idle:o:Native"));
        assert!(matches!(
            handle.snapshot().borrow().session,
            SessionState::Effects { .. }
        ));
        engine.shutdown().await.unwrap();
    })
    .await;
}

/// A session whose source fails after two frames and whose output takes
/// 500 ms to idle: a command that arrives while it is idling `Off` meets a
/// session that ended on its own before the stop.
fn failing_slowly(output: &str) -> Settings {
    settings(
        FakeSourceSettings {
            name: "s".into(),
            fail_after: Some(2),
            ..Default::default()
        },
        FakeOutputSettings {
            name: output.into(),
            idle_delay_ms: 500,
            ..Default::default()
        },
    )
}

/// Off asked while the session ends on its own: the command did what it was
/// asked, so it succeeds, as it does when the end is handled first.
#[tokio::test]
async fn off_asked_while_a_session_fails_on_its_own_succeeds() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(&log, failing_slowly("o"), DEADLINE).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert!(log.wait_for("output.idle:o:Off", 1).await);

        handle.set_target(LightingTarget::Off).await.unwrap();
        let snapshot = handle.snapshot().borrow().clone();
        assert!(
            matches!(snapshot.session, SessionState::Stopped { .. }),
            "{}",
            snapshot.session
        );
        assert_eq!(snapshot.target, LightingTarget::Off);
        assert!(
            snapshot.session.failure().is_none(),
            "{:?}",
            snapshot.session.failure()
        );
        assert_eq!(log.count("output.idle:o:Off"), 1, "idled once");
        engine.shutdown().await.unwrap();
    })
    .await;
}

/// A restart that meets a session failing on its own starts the new session
/// and reports no failure of the old one beside it.
#[tokio::test]
async fn a_restart_while_a_session_fails_on_its_own_reports_only_the_new_session() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(&log, failing_slowly("o"), DEADLINE).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert!(log.wait_for("output.idle:o:Off", 1).await);

        handle
            .replace_settings(revision(&handle), named("s2", "o2"))
            .await
            .unwrap();
        assert!(before(&log, "output.idle:o:Off", "source.open:s2"));
        assert!(log.wait_for("output.write:o2", 1).await);
        let snapshot = handle.snapshot().borrow().clone();
        assert!(
            matches!(snapshot.session, SessionState::ScreenSync),
            "{}",
            snapshot.session
        );
        assert!(
            snapshot.session.failure().is_none(),
            "{:?}",
            snapshot.session.failure()
        );
        engine.shutdown().await.unwrap();
        assert_eq!(log.count("output.idle:o2:Off"), 1);
    })
    .await;
}

/// A restart that meets a source that ended on its own: lighting is complete,
/// as when the end is handled first, so the new settings start nothing.
#[tokio::test]
async fn a_restart_while_the_source_ends_on_its_own_turns_lighting_off() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let ending = settings(
            FakeSourceSettings {
                name: "s".into(),
                end_after: Some(2),
                ..Default::default()
            },
            FakeOutputSettings {
                name: "o".into(),
                idle_delay_ms: 500,
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, ending, DEADLINE).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert!(log.wait_for("output.idle:o:Off", 1).await);

        handle
            .replace_settings(revision(&handle), named("s2", "o"))
            .await
            .unwrap();
        let snapshot = handle.snapshot().borrow().clone();
        assert_eq!(snapshot.target, LightingTarget::Off);
        assert!(
            matches!(snapshot.session, SessionState::Stopped { .. }),
            "{}",
            snapshot.session
        );
        assert!(
            snapshot.session.failure().is_none(),
            "{:?}",
            snapshot.session.failure()
        );
        assert_eq!(log.count("source.open:s2"), 0, "nothing restarted");
        engine.shutdown().await.unwrap();
    })
    .await;
}

/// The lights' own effects asked while the session fails on its own: they
/// get them, and the reply is the effects', not the old failure.
#[tokio::test]
async fn effects_asked_while_a_session_fails_on_its_own_come_on() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let (handle, engine, _) = start(&log, failing_slowly("o"), DEADLINE).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert!(log.wait_for("output.idle:o:Off", 1).await);

        handle.set_target(LightingTarget::Effects).await.unwrap();
        assert!(before(&log, "output.idle:o:Off", "output.idle:o:Native"));
        let snapshot = handle.snapshot().borrow().clone();
        assert!(
            matches!(snapshot.session, SessionState::Effects { .. }),
            "{}",
            snapshot.session
        );
        assert_eq!(snapshot.target, LightingTarget::Effects);
        assert!(
            snapshot.session.failure().is_none(),
            "{:?}",
            snapshot.session.failure()
        );
        engine.shutdown().await.unwrap();
    })
    .await;
}

/// The effects asked after an own end, from lights that cannot idle: the
/// reply is that failure, and the lights are not claimed to show them.
#[tokio::test]
async fn a_failing_idle_after_an_own_end_is_the_reply() {
    within_async(LIMIT, async {
        let log = CallLog::new();
        let unreachable = settings(
            FakeSourceSettings {
                name: "s".into(),
                fail_after: Some(2),
                ..Default::default()
            },
            FakeOutputSettings {
                name: "o".into(),
                idle_delay_ms: 500,
                fail_idle: true,
                ..Default::default()
            },
        );
        let (handle, engine, _) = start(&log, unreachable, DEADLINE).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert!(log.wait_for("output.idle:o:Off", 1).await);

        let error = handle
            .set_target(LightingTarget::Effects)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "fake output cannot idle");
        assert_eq!(log.count("output.idle:o:Native"), 1);
        let snapshot = handle.snapshot().borrow().clone();
        assert!(
            matches!(snapshot.session, SessionState::Stopped { .. }),
            "{}",
            snapshot.session
        );
        assert_eq!(snapshot.target, LightingTarget::Effects);
        engine.shutdown().await.unwrap();
    })
    .await;
}
