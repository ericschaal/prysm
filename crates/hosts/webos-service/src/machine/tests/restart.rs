//! Typed restart and teardown behavior.
use super::*;

fn start() -> Vec<Effect> {
    vec![spawn_full(), video(Video::Unknown)]
}

#[test]
fn the_supervisors_causes_are_classified_once() {
    let cases = [
        (
            Input::SpawnFailed("no such file".into()),
            "cannot start the worker: no such file",
            false,
        ),
        (
            Input::WorkerBroken("bad line".into()),
            "worker link broke: bad line",
            false,
        ),
        (Tick, "worker sent nothing for 15 s", true),
        (Exited(Exit::Code(0)), "worker exited with code 0", true),
        (Exited(Exit::Code(1)), "worker exited with code 1", true),
        (Exited(Exit::Signal(9)), "worker killed by signal 9", true),
    ];
    for (cause, message, restarts) in cases {
        let mut m = full();
        let start = Instant::now();
        m.handle(Pw(Active), start);
        m.handle(initialized(), start);
        m.handle(cause, start + Duration::from_secs(15));
        let state = m.state();
        if restarts {
            assert_eq!(
                state.log.last(),
                Some(&format!("restarting the worker: {message}"))
            );
            assert_eq!(asked(&state), None);
        } else {
            assert_eq!(asked(&state), Some((TRY_AGAIN, message)));
        }
    }
}

/// A worker let go during standby may still hold the capture: the person
/// is still asked to restart the TV after it wakes, whatever they choose
/// or the TV does, until it no longer exists; then their choice applies.
#[test]
fn a_worker_let_go_in_standby_is_asked_about_until_it_is_reaped() {
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
        vec![
            (60_001, Target(LightingTarget::Effects), vec![]),
            (60_002, Pw(Asleep), vec![]),
            (60_003, Pw(Active), vec![]),
        ],
    );
    let later = m.state();
    assert!(matches!(full_state(&later).1, FullWorker::Asking { .. }));
    assert_eq!(asked(&later), asked(&state));
    assert_eq!(m.intent(), LightingTarget::Effects, "the choice is kept");
    script(
        &mut m,
        t0,
        vec![(
            70_000,
            Input::LetGoReaped,
            vec![
                spawn_full(),
                send_target(LightingTarget::Effects, 1),
                video(Video::Unknown),
            ],
        )],
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

/// Initialization from a worker already stopped for a restart does not give the
/// restart back: the restarted worker is the restart.
#[test]
fn a_stopping_workers_initialization_does_not_count() {
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
            (2, initialized(), vec![]),
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
        Some((TRY_AGAIN, "worker exited with code 1"))
    );
}
