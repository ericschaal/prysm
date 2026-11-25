//! Restart-once over several workers: given back by a working state or by
//! an action of the person or the TV, never by time.
use super::*;
use prysm_webos_link::OutputIssue;

fn start() -> Vec<Effect> {
    vec![spawn_full(LightingTarget::Off), video(Video::Unknown)]
}

/// Whether `report`, from the worker that is the restart, gives the
/// restart back: its next exit restarts again instead of asking.
fn gives_the_restart_back(report: FullStatus) -> bool {
    let (mut m, t0) = (full(), Instant::now());
    script(
        &mut m,
        t0,
        vec![
            (0, Pw(Active), start()),
            (1, Exited(Exit::Signal(9)), start()),
            (2, full_status(report), vec![]),
        ],
    );
    m.handle(Exited(Exit::Signal(9)), t0 + Duration::from_millis(3));
    m.state().package.problem().is_none()
}

/// Effects applied, with no failure and no lights issue, is a working
/// state; with the lights not answering, or a failure, it is not.
#[test]
fn effects_earns_the_restart_back() {
    let effects = full_report(LightingTarget::Effects);
    assert!(gives_the_restart_back(effects.clone()));
    assert!(!gives_the_restart_back(FullStatus {
        lights: Some(OutputIssue::Unreachable),
        ..effects.clone()
    }));
    assert!(!gives_the_restart_back(FullStatus {
        failure: Some(failure(Escalation::Unanswered, "no answer")),
        ..effects
    }));
}

/// Off says nothing that works: also what every first status says.
#[test]
fn off_does_not_earn_the_restart_back() {
    assert!(!gives_the_restart_back(full_report(LightingTarget::Off)));
}

#[test]
fn screen_sync_waiting_for_video_does_not_earn_the_restart_back() {
    assert!(!gives_the_restart_back(FullStatus {
        fps: Some(0),
        ..full_report(LightingTarget::ScreenSync)
    }));
}

fn capturing() -> Input {
    full_status(FullStatus {
        capturing: true,
        ..full_report(LightingTarget::Off)
    })
}

/// Two exits before a frame: one restart, then the person is asked; their
/// choice starts a worker with its restart again.
#[test]
fn a_restart_that_did_not_help_asks_the_person() {
    let (mut m, t0) = (full(), Instant::now());
    script(
        &mut m,
        t0,
        vec![
            (0, Pw(Active), start()),
            (1_000, Exited(Exit::Signal(9)), start()),
            (2_000, status(LightingTarget::Off), vec![]),
            (3_000, Exited(Exit::Signal(9)), vec![]),
            (3_600_000, Tick, vec![]),
        ],
    );
    let state = m.state();
    assert!(matches!(full_state(&state).1, FullWorker::Asking { .. }));
    assert_eq!(
        asked(&state),
        Some((
            TRY_AGAIN,
            "again after a restart: worker killed by signal 9"
        ))
    );
    assert_eq!(
        state.log,
        ["restarting the worker: worker killed by signal 9"]
    );
    script(
        &mut m,
        t0,
        vec![
            (
                3_600_001,
                Target(LightingTarget::Effects),
                vec![spawn_full(LightingTarget::Effects), video(Video::Unknown)],
            ),
            (
                3_600_002,
                Exited(Exit::Code(1)),
                vec![spawn_full(LightingTarget::Effects), video(Video::Unknown)],
            ),
        ],
    );
}

/// A fault after each first frame restarts each time, unpaced: every
/// restart followed a working state.
#[test]
fn a_frame_gives_the_restart_back_each_time() {
    let (mut m, t0) = (full(), Instant::now());
    let mut steps = vec![(0, Pw(Active), start())];
    for at in [1_000, 2_000, 3_000] {
        steps.push((at, capturing(), vec![]));
        steps.push((at + 1, Exited(Exit::Signal(11)), start()));
    }
    script(&mut m, t0, steps);
    let state = m.state();
    assert_eq!(state.package.problem(), None);
    assert_eq!(state.log.len(), 3, "each restart is in the tail");
}

/// Standby gives the restart back: a fault after the next wake restarts
/// again instead of asking.
#[test]
fn standby_gives_the_restart_back() {
    let (mut m, t0) = (full(), Instant::now());
    script(
        &mut m,
        t0,
        vec![
            (0, Pw(Active), start()),
            (1_000, Exited(Exit::Signal(9)), start()),
            (
                2_000,
                Pw(Asleep),
                vec![video(Video::Unavailable), Effect::Terminate],
            ),
            (2_100, Exited(Exit::Code(0)), vec![]),
            (3_000, Pw(Active), start()),
            (4_000, Exited(Exit::Signal(9)), start()),
        ],
    );
    assert_eq!(m.state().package.problem(), None);
}

/// A worker let go during standby may still hold the capture: the person
/// is still asked to restart the TV after it wakes, until they choose.
#[test]
fn a_worker_let_go_in_standby_is_asked_about_at_wake() {
    let (mut m, t0) = (full(), Instant::now());
    script(
        &mut m,
        t0,
        vec![
            (0, Pw(Active), start()),
            (
                1_000,
                Pw(Asleep),
                vec![video(Video::Unavailable), Effect::Terminate],
            ),
            (9_000, Tick, vec![Effect::Kill]),
            (17_000, Tick, vec![Effect::Abandon]),
            (60_000, Pw(Active), vec![]),
        ],
    );
    let state = m.state();
    assert!(matches!(full_state(&state).1, FullWorker::Asking { .. }));
    assert_eq!(
        asked(&state),
        Some((
            Asked::Ask(Fix::RestartTv),
            "worker not reaped 8 s after its kill"
        ))
    );
    script(
        &mut m,
        t0,
        vec![(60_001, Target(LightingTarget::Off), start())],
    );
}

/// Slim: a capture that could not confirm its release ends the worker,
/// which is restarted once; capture on answers what the person is asked.
#[test]
fn a_slim_worker_is_restarted_once_then_asks() {
    let mut m = slim(Preferences {
        autostart: true,
        ..slim_prefs()
    });
    let t0 = Instant::now();
    let start = || vec![spawn_slim(&receiver()), video(Video::Unknown)];
    let unconfirmed = || failed_with(Escalation::Restart, "release unconfirmed");
    script(
        &mut m,
        t0,
        vec![
            (0, Pw(Active), start()),
            (1, unconfirmed(), vec![]),
            (2, Exited(Exit::Code(1)), start()),
            (3, unconfirmed(), vec![]),
            (4, Exited(Exit::Code(1)), vec![]),
        ],
    );
    assert_eq!(
        asked(&m.state()),
        Some((TRY_AGAIN, "again after a restart: release unconfirmed"))
    );
    script(&mut m, t0, vec![(5, Enabled(true), start())]);
}

/// Frames from a worker already stopped for a restart do not give the
/// restart back: the restarted worker is the restart.
#[test]
fn a_stopping_workers_frames_do_not_count() {
    let (mut m, t0) = (full(), Instant::now());
    let fault = full_status(FullStatus {
        failure: Some(failure(Escalation::Restart, "release unconfirmed")),
        ..full_report(LightingTarget::Off)
    });
    script(
        &mut m,
        t0,
        vec![
            (0, Pw(Active), start()),
            (1, fault, vec![Effect::Terminate]),
            (2, capturing(), vec![]),
            (3, Exited(Exit::Code(1)), start()),
            (4, Exited(Exit::Code(1)), vec![]),
        ],
    );
    assert!(matches!(
        full_state(&m.state()).1,
        FullWorker::Asking { .. }
    ));
    assert_eq!(
        asked(&m.state()),
        Some((
            TRY_AGAIN,
            "again after a restart: worker exited with code 1"
        ))
    );
}
