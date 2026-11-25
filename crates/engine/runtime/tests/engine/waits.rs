//! Lights that do not answer the request putting them off or in their own
//! effects: waited for, as screen sync waits for their open. Under tokio's
//! paused clock: the fakes' idles run on blocking threads, which hold it.
use crate::common::*;
use prysm_ports::{DeviceReport, Escalation, LightingTarget, OutputIssue, SessionState, Snapshot};
use prysm_runtime::fakes::{CallLog, FakeIdle, FakeOutputSettings, FakeSourceSettings};
use prysm_runtime::{EngineHandle, UNANSWERED_INTERVAL};

/// The event that makes the lights of [`silent`] answer.
const ANSWER: &str = "test.answer";

/// Source `s` and output `o`, whose idles the lights answer as `idle` says.
fn idling(idle: FakeIdle) -> prysm_ports::Settings {
    let source = FakeSourceSettings {
        name: "s".into(),
        ..Default::default()
    };
    let output = FakeOutputSettings {
        name: "o".into(),
        idle,
        ..Default::default()
    };
    settings(source, output)
}

/// Lights that answer no idle until [`ANSWER`].
fn silent() -> prysm_ports::Settings {
    idling(FakeIdle::UnansweredUntil(ANSWER.into()))
}

/// The target taken, no failure and the lights unreachable.
fn waiting(snapshot: &Snapshot, target: LightingTarget) -> bool {
    snapshot.target == target
        && matches!(snapshot.session, SessionState::Stopped { failure: None })
        && snapshot.device.issue == Some(OutputIssue::Unreachable)
}

/// Let the paused clock run `by`, then a round trip through the engine's
/// loop that asks nothing of the lights.
async fn after(handle: &EngineHandle, by: std::time::Duration) {
    tokio::time::advance(by).await;
    handle.set_calibration(None).await.unwrap();
}

/// From a wait for `target`, whose request `event` records, tried `tried`
/// times so far: no try within the interval and one at it, published as
/// before; once the lights answer, the next try reaches the target and
/// the issue clears; then nothing is tried. The clock is paused.
async fn waits_until_answered(
    handle: &EngineHandle,
    log: &CallLog,
    target: LightingTarget,
    event: &str,
    tried: usize,
) {
    assert!(waiting(&handle.snapshot().borrow(), target));
    let mut snapshot = handle.snapshot();
    snapshot.mark_unchanged();
    after(handle, UNANSWERED_INTERVAL / 2).await;
    assert_eq!(log.count(event), tried, "not within the interval");
    after(handle, UNANSWERED_INTERVAL / 2).await;
    assert!(log.wait_for(event, tried + 1).await, "at the interval");
    after(handle, std::time::Duration::ZERO).await;
    assert!(!snapshot.has_changed().unwrap(), "a wait that goes on");

    log.record(ANSWER);
    tokio::time::advance(UNANSWERED_INTERVAL).await;
    let answered = snapshot_until(handle, |s| {
        s.target == target && s.session.reached(target) && s.device == DeviceReport::default()
    })
    .await;
    assert!(answered.session.failure().is_none());
    assert_eq!(log.count(event), tried + 2);
    after(handle, UNANSWERED_INTERVAL * 3).await;
    assert_eq!(log.count(event), tried + 2, "nothing after the answer");
}

#[tokio::test(start_paused = true)]
async fn unanswered_effects_are_waited_for_until_the_lights_answer() {
    let log = CallLog::new();
    let (handle, engine, _) = start(&log, silent(), DEADLINE).await;
    handle.set_target(LightingTarget::Effects).await.unwrap();
    assert_eq!(log.count("output.idle:o:Native"), 1);
    let effects = LightingTarget::Effects;
    waits_until_answered(&handle, &log, effects, "output.idle:o:Native", 1).await;
    engine.shutdown().await.unwrap();
}

/// Off after effects the lights did not answer: the command ends that wait
/// at once, its own request is tried at once and waited for in turn.
#[tokio::test(start_paused = true)]
async fn unanswered_off_is_waited_for_and_a_new_target_ends_a_wait_at_once() {
    let log = CallLog::new();
    let (handle, engine, _) = start(&log, silent(), DEADLINE).await;
    handle.set_target(LightingTarget::Effects).await.unwrap();
    handle.set_target(LightingTarget::Off).await.unwrap();
    assert_eq!(log.count("output.idle:o:Off"), 1, "tried at once");
    let off = LightingTarget::Off;
    waits_until_answered(&handle, &log, off, "output.idle:o:Off", 1).await;
    assert_eq!(log.count("output.idle:o:Native"), 1, "that wait ended");
    engine.shutdown().await.unwrap();
}

/// Screen sync stopped for `Off` or the lights' own effects: the stop's idle
/// they do not answer is waited for as the same request without a session.
/// The session runs and stops on the running clock.
#[tokio::test]
async fn a_stop_the_lights_do_not_answer_is_waited_for() {
    for (target, event) in [
        (LightingTarget::Off, "output.idle:o:Off"),
        (LightingTarget::Effects, "output.idle:o:Native"),
    ] {
        let log = CallLog::new();
        let (handle, engine, _) = start(&log, silent(), DEADLINE).await;
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        assert!(log.wait_for("output.write:o", 1).await);
        handle.set_target(target).await.unwrap();
        assert_eq!(log.count(event), 1, "the held output's idle");
        tokio::time::pause();
        waits_until_answered(&handle, &log, target, event, 1).await;
        tokio::time::resume();
        engine.shutdown().await.unwrap();
    }
}

/// Screen sync, or a settings save, during a wait: it ends at once, and
/// what the command asks is done now.
#[tokio::test(start_paused = true)]
async fn a_command_ends_a_wait_at_once() {
    let log = CallLog::new();
    let (handle, engine, _) = start(&log, silent(), DEADLINE).await;
    handle.set_target(LightingTarget::Effects).await.unwrap();
    let mut saved = silent();
    saved.processing.brightness_percent = 40;
    handle
        .replace_settings(revision(&handle), saved)
        .await
        .unwrap();
    assert_eq!(log.count("output.idle:o:Native"), 2, "tried again at once");
    assert!(waiting(
        &handle.snapshot().borrow(),
        LightingTarget::Effects
    ));

    handle.set_target(LightingTarget::ScreenSync).await.unwrap();
    let snapshot = handle.snapshot().borrow().clone();
    assert!(matches!(snapshot.session, SessionState::ScreenSync));
    assert_eq!(snapshot.device, DeviceReport::default());
    assert_eq!(log.count("source.open:s"), 1);
    after(&handle, UNANSWERED_INTERVAL * 2).await;
    assert_eq!(log.count("output.idle:o:Native"), 2, "that wait ended");
    tokio::time::resume();
    // The session's own idle at shutdown is answered.
    log.record(ANSWER);
    engine.shutdown().await.unwrap();
}

/// An output saved during a wait for `Off` is tried at once, whatever
/// changed: it may be the same lights, which the wait never reached.
#[tokio::test(start_paused = true)]
async fn an_output_saved_during_a_wait_for_off_is_tried_at_once() {
    let log = CallLog::new();
    let (handle, engine, _) = start(&log, silent(), DEADLINE).await;
    handle.set_target(LightingTarget::Effects).await.unwrap();
    handle.set_target(LightingTarget::Off).await.unwrap();
    let mut saved = silent();
    saved.output = Some(prysm_ports::AdapterSettings::new(
        "fake",
        &FakeOutputSettings {
            name: "o".into(),
            idle: FakeIdle::UnansweredUntil(ANSWER.into()),
            brightness: Some(50),
            ..Default::default()
        },
    ));
    handle
        .replace_settings(revision(&handle), saved)
        .await
        .unwrap();
    assert_eq!(log.count("output.idle:o:Off"), 2, "tried at once");
    assert!(waiting(&handle.snapshot().borrow(), LightingTarget::Off));
    engine.shutdown().await.unwrap();
}

/// Screen sync whose source delivers no frame opens no output: `Off` after
/// it still puts in `Off` lights left in their effects, or waited for. The
/// session runs and stops on the running clock.
#[tokio::test]
async fn off_after_screen_sync_without_a_frame_reaches_lights_left_on() {
    for (idle, waits) in [
        (FakeIdle::Idles, false),
        (FakeIdle::UnansweredUntil(ANSWER.into()), true),
    ] {
        let log = CallLog::new();
        let source = FakeSourceSettings {
            name: "s".into(),
            quiet_after: Some(0),
            ..Default::default()
        };
        let output = FakeOutputSettings {
            name: "o".into(),
            idle,
            ..Default::default()
        };
        let (handle, engine, _) = start(&log, settings(source, output), DEADLINE).await;
        handle.set_target(LightingTarget::Effects).await.unwrap();
        handle.set_target(LightingTarget::ScreenSync).await.unwrap();
        handle.set_target(LightingTarget::Off).await.unwrap();
        assert_eq!(log.count("output.idle:o:Off"), 1, "waits: {waits}");
        let snapshot = handle.snapshot().borrow().clone();
        assert_eq!(waiting(&snapshot, LightingTarget::Off), waits);
        assert!(snapshot.session.failure().is_none());
        log.record(ANSWER);
        engine.shutdown().await.unwrap();
    }
}

/// A shutdown during a wait tries the request once more and waits for
/// nothing: the lights' silence fails no shutdown.
#[tokio::test(start_paused = true)]
async fn a_shutdown_during_a_wait_tries_once_and_completes() {
    let log = CallLog::new();
    let (handle, engine, _) = start(&log, silent(), DEADLINE).await;
    handle.set_target(LightingTarget::Effects).await.unwrap();
    engine.shutdown().await.unwrap();
    assert_eq!(log.count("output.idle:o:Native"), 2);
    let snapshot = handle.snapshot().borrow().clone();
    assert!(snapshot.session.failure().is_none());
    assert_eq!(snapshot.device, DeviceReport::default());
}

/// A shutdown of screen sync whose idle the lights do not answer: that
/// fails no shutdown either.
#[tokio::test]
async fn a_shutdown_the_lights_do_not_answer_completes() {
    let log = CallLog::new();
    let (handle, engine, _) = start(&log, silent(), DEADLINE).await;
    handle.set_target(LightingTarget::ScreenSync).await.unwrap();
    assert!(log.wait_for("output.write:o", 1).await);
    engine.shutdown().await.unwrap();
    assert_eq!(log.count("output.idle:o:Off"), 1);
}

/// Lights that refuse the request, rather than not answering it, fail it
/// as before: nothing is tried again on its own.
#[tokio::test(start_paused = true)]
async fn any_other_failure_is_not_waited_for() {
    let log = CallLog::new();
    let (handle, engine, _) = start(&log, idling(FakeIdle::Fails), DEADLINE).await;
    let failed = handle.set_target(LightingTarget::Effects).await;
    assert_eq!(failed.unwrap_err().escalation(), Escalation::FAIL_FAST);
    let snapshot = handle.snapshot().borrow().clone();
    assert!(snapshot.session.failure().is_some());
    assert_eq!(snapshot.device, DeviceReport::default());
    after(&handle, UNANSWERED_INTERVAL * 3).await;
    assert_eq!(log.count("output.idle:o:Native"), 1);
    engine.shutdown().await.unwrap();
}
