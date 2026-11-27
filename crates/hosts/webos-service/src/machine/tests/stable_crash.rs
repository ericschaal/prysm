use super::*;

fn start() -> Vec<Effect> {
    vec![spawn_full(), video(Video::Unknown)]
}

#[test]
fn heartbeat_does_not_establish_stability() {
    let (mut machine, now) = (full(), Instant::now());
    script(
        &mut machine,
        now,
        vec![
            (0, Pw(Active), start()),
            (1, status(LightingTarget::Off), vec![]),
            (2, Exited(Exit::Code(1)), vec![]),
            (60_000, Tick, vec![]),
        ],
    );
    assert_eq!(
        asked(&machine.state()),
        Some((TRY_AGAIN, "worker exited with code 1"))
    );
}

#[test]
fn each_worker_can_restart_after_explicit_initialization_even_without_frames() {
    let (mut machine, now) = (full(), Instant::now());
    script(
        &mut machine,
        now,
        vec![
            (0, Pw(Active), start()),
            (1, initialized(), vec![]),
            (1, Exited(Exit::Code(1)), start()),
            (2, initialized(), vec![]),
            (2, Exited(Exit::Signal(9)), start()),
            (3, Exited(Exit::Code(2)), vec![]),
        ],
    );
    assert_eq!(
        asked(&machine.state()),
        Some((TRY_AGAIN, "worker exited with code 2"))
    );
}

#[test]
fn terminal_adapter_decision_precedes_generic_recovery() {
    let (mut machine, now) = (full(), Instant::now());
    let capability = Asked::Ask(Fix::Unsupported);
    script(
        &mut machine,
        now,
        vec![
            (0, Pw(Active), start()),
            (1, initialized(), vec![]),
            (
                2,
                failed_with(Escalation::Ask(Fix::Unsupported), "unsupported source"),
                vec![],
            ),
            (3, failed_with(Escalation::Restart, "late failure"), vec![]),
            (4, Exited(Exit::Code(1)), vec![]),
            (60_000, Tick, vec![]),
        ],
    );
    assert_eq!(
        asked(&machine.state()),
        Some((capability, "unsupported source"))
    );
}

#[test]
fn explicit_process_recovery_can_take_precedence_during_initialization() {
    let (mut machine, now) = (full(), Instant::now());
    script(
        &mut machine,
        now,
        vec![
            (0, Pw(Active), start()),
            (
                1,
                failed_with(Escalation::Restart, "failed to release capture"),
                vec![],
            ),
            (2, Exited(Exit::Code(1)), start()),
            (
                3,
                failed_with(Escalation::Ask(Fix::SetUp), "capture unavailable"),
                vec![],
            ),
            (4, Exited(Exit::Code(1)), vec![]),
        ],
    );
    assert_eq!(
        asked(&machine.state()),
        Some((Asked::Ask(Fix::SetUp), "capture unavailable"))
    );
}

#[test]
fn stable_recovery_waits_for_reaping_and_pause_can_supersede_it() {
    let silence = u64::try_from(SILENCE.as_millis()).unwrap();
    let kill = u64::try_from(KILL_AFTER.as_millis()).unwrap();
    for pause in [false, true] {
        let (mut machine, now) = (slim(slim_prefs()), Instant::now());
        let start = vec![spawn_slim(&receiver()), video(Video::Unknown)];
        script(
            &mut machine,
            now,
            vec![
                (0, Enabled(true), vec![]),
                (0, Pw(Active), start.clone()),
                (0, initialized(), vec![]),
                (silence, Tick, vec![Effect::Terminate]),
                (silence + kill, Tick, vec![Effect::Kill]),
                (silence + 2 * kill, Tick, vec![Effect::Abandon]),
                (silence + 2 * kill + 1, Tick, vec![]),
            ],
        );
        let release = silence + 2 * kill + 2;
        if pause {
            script(&mut machine, now, vec![(release, Enabled(false), vec![])]);
        }
        script(
            &mut machine,
            now,
            vec![(
                release,
                Input::LetGoReaped,
                if pause { vec![] } else { start },
            )],
        );
    }
}

#[test]
fn the_last_typed_capture_status_wins_when_a_stable_worker_exits() {
    let (mut machine, now) = (full(), Instant::now());
    let status = full_status(FullStatus {
        failure: Some(failure(
            Escalation::Ask(Fix::Unsupported),
            "capture is blocked",
        )),
        ..full_report(LightingTarget::Off)
    });
    script(
        &mut machine,
        now,
        vec![
            (0, Pw(Active), start()),
            (0, initialized(), vec![]),
            (1, status, vec![]),
            (2, Exited(Exit::Code(1)), vec![]),
        ],
    );
    assert_eq!(
        asked(&machine.state()),
        Some((Asked::Ask(Fix::Unsupported), "capture is blocked"))
    );
}

#[test]
fn a_stale_boot_failure_cannot_stop_the_latest_queued_operation() {
    let (mut machine, now) = (full(), Instant::now());
    let boot_failure = full_status(FullStatus {
        failure: Some(failure(Escalation::Restart, "old operation")),
        ..full_report(LightingTarget::Off)
    });
    script(
        &mut machine,
        now,
        vec![
            (0, Input::Resume, vec![]),
            (
                0,
                Pw(Active),
                vec![
                    spawn_full(),
                    Effect::Send(Command::Resume { sequence: 1 }),
                    video(Video::Unknown),
                ],
            ),
            (1, boot_failure, vec![]),
        ],
    );
    assert_eq!(asked(&machine.state()), None);
}

#[test]
fn a_crash_replacement_resumes_screen_sync_without_an_explicit_takeover() {
    let (mut machine, now) = (full(), Instant::now());
    script(
        &mut machine,
        now,
        vec![
            (0, Pw(Active), start()),
            (
                1,
                Target(LightingTarget::ScreenSync),
                vec![send_target(LightingTarget::ScreenSync, 1)],
            ),
            (2, status_after(LightingTarget::ScreenSync, 1), vec![]),
            (2, initialized(), vec![]),
            (
                3,
                Exited(Exit::Signal(9)),
                vec![
                    spawn_full(),
                    Effect::Send(Command::RecoverScreenSync { sequence: 1 }),
                    video(Video::Unknown),
                ],
            ),
        ],
    );
}

#[test]
fn a_new_target_during_teardown_supersedes_screen_sync_recovery() {
    let silence = u64::try_from(SILENCE.as_millis()).unwrap();
    for target in [LightingTarget::Effects, LightingTarget::Off] {
        let (mut machine, now) = (full(), Instant::now());
        script(
            &mut machine,
            now,
            vec![
                (0, Pw(Active), start()),
                (
                    0,
                    Target(LightingTarget::ScreenSync),
                    vec![send_target(LightingTarget::ScreenSync, 1)],
                ),
                (0, initialized(), vec![]),
                (silence, Tick, vec![Effect::Terminate]),
                (silence + 1, Target(target), vec![]),
                (
                    silence + 2,
                    Exited(Exit::Code(1)),
                    vec![spawn_full(), send_target(target, 1), video(Video::Unknown)],
                ),
            ],
        );
    }
}
