//! Every machine rule, as scripted inputs at explicit times.
use super::*;
use prysm_webos_link::{FailureKind, PrysmAddress, SlimEncoding, SlimLink, WorkerFailure};
use serde_json::json;

fn full() -> Machine {
    full_with(Preferences::default())
}

fn full_with(preferences: Preferences) -> Machine {
    Machine::new(
        Spec::Full {
            settings: "/s.json".into(),
            listen: "0.0.0.0:8787".parse().unwrap(),
        },
        preferences,
    )
}

fn receiver() -> PrysmAddress {
    "http://192.168.1.20:8787".parse().unwrap()
}

fn slim_prefs() -> Preferences {
    Preferences {
        instance: Some(receiver()),
        ..Preferences::default()
    }
}

fn slim(preferences: Preferences) -> Machine {
    Machine::new(Spec::Slim, preferences)
}

fn spawn_full(target: LightingTarget) -> Effect {
    Effect::Spawn(
        FullArgs {
            settings: "/s.json".into(),
            listen: "0.0.0.0:8787".parse().unwrap(),
            target,
        }
        .to_args(),
    )
}

fn spawn_slim(receiver: &PrysmAddress) -> Effect {
    Effect::Spawn(
        SlimArgs {
            prysm: receiver.clone(),
        }
        .to_args(),
    )
}

/// Another receiver, for preference changes.
fn other_receiver() -> PrysmAddress {
    "https://prysm.example.com".parse().unwrap()
}

fn video(video: Video) -> Effect {
    Effect::Send(Command::Video { video })
}

fn send_target(target: LightingTarget) -> Effect {
    Effect::Send(Command::Target { target })
}

/// The published failure's message.
fn message(state: &State) -> Option<&str> {
    state
        .failure
        .as_ref()
        .map(|failure| failure.message.as_str())
}

/// The published failure's kind.
fn kind(state: &State) -> Option<FailureKind> {
    state.failure.as_ref().map(|failure| failure.kind)
}

/// A full worker's report of `target`: no frames, no portal, no failure.
fn full_report(target: LightingTarget) -> FullStatus {
    FullStatus {
        capturing: false,
        fps: None,
        target,
        portal: None,
        failure: None,
    }
}

fn reported(status: WorkerStatus) -> Input {
    Input::Worker(Event::Status(status))
}

/// A full worker reports `target` and nothing else.
fn status(target: LightingTarget) -> Input {
    full_status(full_report(target))
}

fn full_status(status: FullStatus) -> Input {
    reported(WorkerStatus::Full(status))
}

fn slim_status(status: SlimStatus) -> Input {
    reported(WorkerStatus::Slim(status))
}

/// A slim worker reports `link`, no frames.
fn slim_link(link: SlimLink) -> Input {
    slim_status(SlimStatus {
        capturing: false,
        link,
    })
}

/// A slim worker reports `failure`: a wait for the receiver, or failed.
fn slim_failure(kind: FailureKind, message: &str) -> Input {
    let failure = WorkerFailure::new(kind, message);
    slim_link(match kind {
        FailureKind::Receiver => SlimLink::Waiting { failure },
        _ => SlimLink::Failed { failure },
    })
}

/// The published full package: its target and worker.
fn full_state(state: &State) -> (LightingTarget, &FullWorker) {
    match &state.package {
        state::Package::Full { target, worker } => (*target, worker),
        state::Package::Slim { .. } => panic!("a slim state: {state:?}"),
    }
}

/// The published slim package: whether enabled, and its worker.
fn slim_state(state: &State) -> (bool, &SlimWorker) {
    match &state.package {
        state::Package::Slim { enabled, worker } => (*enabled, worker),
        state::Package::Full { .. } => panic!("a full state: {state:?}"),
    }
}

/// The published full worker is running with frames arriving.
fn full_capturing(state: &State) -> bool {
    matches!(
        full_state(state).1,
        FullWorker::Running {
            capturing: true,
            ..
        }
    )
}

/// Feed `(milliseconds, input, expected effects without Publish)` in order.
fn script(machine: &mut Machine, t0: Instant, steps: Vec<(u64, Input, Vec<Effect>)>) {
    for (index, (at, input, expected)) in steps.into_iter().enumerate() {
        let shown = format!("{input:?}");
        let effects: Vec<_> = machine
            .handle(input, t0 + Duration::from_millis(at))
            .into_iter()
            .filter(|effect| *effect != Effect::Publish)
            .collect();
        assert_eq!(effects, expected, "step {index} at {at} ms: {shown}");
    }
}

use crate::state::Power::{Active, Asleep, ScreenSaver, Unknown};
use Input::{Enabled, Power as Pw, Target, Tick, Video as V, WorkerExited as Exited};

#[test]
fn full_runs_while_awake_and_gates_video_on_active() {
    let (mut m, t) = (full(), Instant::now());
    script(
        &mut m,
        t,
        vec![
            (0, Pw(Unknown), vec![]),
            (
                0,
                Pw(Active),
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
            (1, V(Video::Available), vec![video(Video::Available)]),
            (2, V(Video::Available), vec![]),
            (3, Pw(ScreenSaver), vec![video(Video::Unavailable)]),
            (4, V(Video::Unavailable), vec![]),
            (5, V(Video::Available), vec![]),
            (6, Pw(Active), vec![video(Video::Available)]),
            (
                7,
                Pw(Asleep),
                vec![video(Video::Unavailable), Effect::Terminate],
            ),
            (8, Pw(Active), vec![]),
            (
                9,
                Exited(Exit::Code(0)),
                vec![spawn_full(LightingTarget::Off), video(Video::Available)],
            ),
        ],
    );
    assert!(matches!(
        full_state(&m.state(t)).1,
        FullWorker::Running { .. }
    ));
}

#[test]
fn full_autostart_starts_screen_sync_and_the_target_is_sent() {
    let mut m = full_with(Preferences {
        autostart: true,
        ..Preferences::default()
    });
    let t = Instant::now();
    assert_eq!(full_state(&m.state(t)).0, LightingTarget::ScreenSync);
    script(
        &mut m,
        t,
        vec![
            (
                0,
                Pw(Active),
                vec![
                    spawn_full(LightingTarget::ScreenSync),
                    video(Video::Unknown),
                ],
            ),
            (
                10,
                Target(LightingTarget::Off),
                vec![send_target(LightingTarget::Off)],
            ),
            // An older report within the echo window does not undo it.
            (20, status(LightingTarget::ScreenSync), vec![]),
        ],
    );
    assert_eq!(full_state(&m.state(t)).0, LightingTarget::Off);
    script(
        &mut m,
        t,
        vec![
            (30, status(LightingTarget::Off), vec![]),
            // The portal turned the lights' own effects on.
            (40, status(LightingTarget::Effects), vec![]),
        ],
    );
    assert_eq!(full_state(&m.state(t)).0, LightingTarget::Effects);
    script(
        &mut m,
        t,
        vec![
            // The app's target is sent as chosen.
            (
                50,
                Target(LightingTarget::ScreenSync),
                vec![send_target(LightingTarget::ScreenSync)],
            ),
            (
                60,
                Target(LightingTarget::Off),
                vec![send_target(LightingTarget::Off)],
            ),
            (2059, status(LightingTarget::ScreenSync), vec![]),
        ],
    );
    assert_eq!(full_state(&m.state(t)).0, LightingTarget::Off);
    // After the echo window the worker's report wins again.
    script(
        &mut m,
        t,
        vec![(2060, status(LightingTarget::ScreenSync), vec![])],
    );
    assert_eq!(full_state(&m.state(t)).0, LightingTarget::ScreenSync);
}

#[test]
fn full_remembers_the_target_for_the_next_worker() {
    let (mut m, t) = (full(), Instant::now());
    script(
        &mut m,
        t,
        vec![
            (0, Target(LightingTarget::ScreenSync), vec![]),
            (
                0,
                Pw(Active),
                vec![
                    spawn_full(LightingTarget::ScreenSync),
                    video(Video::Unknown),
                ],
            ),
            (5, status(LightingTarget::Effects), vec![]),
            (
                6,
                Pw(Asleep),
                vec![video(Video::Unavailable), Effect::Terminate],
            ),
            (7, Exited(Exit::Code(0)), vec![]),
            (
                8,
                Pw(Active),
                vec![spawn_full(LightingTarget::Effects), video(Video::Unknown)],
            ),
        ],
    );
}

#[test]
fn slim_needs_a_receiver_and_enable() {
    let (mut m, t) = (slim(Preferences::default()), Instant::now());
    script(
        &mut m,
        t,
        vec![
            (0, Pw(Active), vec![]),
            (1, Enabled(true), vec![]),
            (
                2,
                Input::Preferences(slim_prefs()),
                vec![spawn_slim(&receiver()), video(Video::Unknown)],
            ),
            (3, Enabled(false), vec![Effect::Terminate]),
            (4, Exited(Exit::Code(0)), vec![]),
        ],
    );
    assert_eq!(slim_state(&m.state(t)), (false, &SlimWorker::Stopped));
    // Lighting is never sent to a slim worker.
    let mut m = slim(Preferences {
        autostart: true,
        ..slim_prefs()
    });
    script(
        &mut m,
        t,
        vec![(
            0,
            Pw(Active),
            vec![spawn_slim(&receiver()), video(Video::Unknown)],
        )],
    );
    script(&mut m, t, vec![(1, Enabled(true), vec![])]);
}

#[test]
fn slim_receiver_change_restarts_without_backoff_after_reaping() {
    let mut m = slim(Preferences {
        autostart: true,
        ..slim_prefs()
    });
    let t = Instant::now();
    let fast = other_receiver();
    script(
        &mut m,
        t,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_slim(&receiver()), video(Video::Unknown)],
            ),
            (
                1,
                Input::Preferences(Preferences {
                    instance: Some(fast.clone()),
                    ..slim_prefs()
                }),
                vec![Effect::Terminate],
            ),
            // Nothing starts before the old worker is reaped.
            (2, Pw(ScreenSaver), vec![]),
            (3, Pw(Active), vec![]),
            (4, Tick, vec![]),
            (
                5,
                Exited(Exit::Code(0)),
                vec![spawn_slim(&fast), video(Video::Unknown)],
            ),
            // Autostart alone does not restart.
            (
                6,
                Input::Preferences(Preferences {
                    instance: Some(fast.clone()),
                    autostart: false,
                }),
                vec![],
            ),
        ],
    );
    assert_eq!(m.state(t).failure, None);
}

#[test]
fn failures_back_off_doubling_to_thirty_seconds() {
    let (mut m, t0) = (full(), Instant::now());
    script(
        &mut m,
        t0,
        vec![(
            0,
            Pw(Active),
            vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
        )],
    );
    let mut now = 0;
    for delay in [1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000] {
        script(
            &mut m,
            t0,
            vec![
                (now, Exited(Exit::Code(1)), vec![]),
                (now + delay - 1, Tick, vec![]),
                (
                    now + delay,
                    Tick,
                    vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
                ),
            ],
        );
        now += delay;
    }
    assert_eq!(message(&m.state(t0)), Some("worker failed"));
}

#[test]
fn retrying_publishes_the_seconds_until_the_next_start() {
    let (mut m, t0) = (full(), Instant::now());
    let start = vec![spawn_full(LightingTarget::Off), video(Video::Unknown)];
    script(
        &mut m,
        t0,
        vec![
            (0, Pw(Active), start.clone()),
            (0, Exited(Exit::Code(1)), vec![]),
            (1_000, Tick, start.clone()),
            (1_000, Exited(Exit::Code(1)), vec![]),
            (3_000, Tick, start),
        ],
    );
    // The third failure waits 4 s; the state carries what is left, rounded up.
    let effects = m.handle(Exited(Exit::Code(1)), t0 + Duration::from_millis(3_000));
    assert!(effects.contains(&Effect::Publish));
    let at = |ms| {
        full_state(&m.state(t0 + Duration::from_millis(ms)))
            .1
            .clone()
    };
    assert_eq!(at(3_000), FullWorker::Retrying { seconds: 4 });
    assert_eq!(at(3_001), FullWorker::Retrying { seconds: 4 });
    assert_eq!(at(4_000), FullWorker::Retrying { seconds: 3 });
    assert_eq!(at(6_999), FullWorker::Retrying { seconds: 1 });
    assert_eq!(
        serde_json::to_value(m.state(t0 + Duration::from_millis(3_500))).unwrap()["worker"],
        json!({ "phase": "retrying", "seconds": 4 })
    );
    // Time alone does not publish: the app counts down by itself.
    let effects = m.handle(Tick, t0 + Duration::from_millis(5_000));
    assert!(!effects.contains(&Effect::Publish));
}

#[test]
fn a_healthy_minute_resets_backoff_and_clears_the_failure() {
    let (mut m, t0) = (full(), Instant::now());
    script(
        &mut m,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
            (1, Exited(Exit::Code(1)), vec![]),
            (
                1_001,
                Tick,
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
            (2_000, Exited(Exit::Code(1)), vec![]),
            (
                4_000,
                Tick,
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
        ],
    );
    assert!(m.state(t0).failure.is_some());
    for at in (14_000..=64_000).step_by(10_000) {
        script(&mut m, t0, vec![(at, status(LightingTarget::Off), vec![])]);
        script(&mut m, t0, vec![(at, Tick, vec![])]);
    }
    assert_eq!(m.state(t0).failure, None);
    script(
        &mut m,
        t0,
        vec![
            (65_000, Exited(Exit::Code(3)), vec![]),
            (
                66_000,
                Tick,
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
        ],
    );
}

#[test]
fn silence_is_a_hang_then_term_then_one_kill() {
    let (mut m, t0) = (full(), Instant::now());
    script(
        &mut m,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
            (10_000, status(LightingTarget::Off), vec![]),
            (24_999, Tick, vec![]),
            (25_000, Tick, vec![Effect::Terminate]),
            // A late line from the stopping worker changes nothing.
            (26_000, status(LightingTarget::Off), vec![]),
            (32_999, Tick, vec![]),
            (33_000, Tick, vec![Effect::Kill]),
            (40_000, Tick, vec![]),
            (40_001, Exited(Exit::Signal(9)), vec![]),
        ],
    );
    let state = m.state(t0);
    assert!(matches!(full_state(&state).1, FullWorker::Retrying { .. }));
    assert_eq!(message(&state), Some("worker sent nothing for 15 s"));
    script(
        &mut m,
        t0,
        vec![(
            41_001,
            Tick,
            vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
        )],
    );
}

#[test]
fn faults_broken_links_and_spawn_failures_retry() {
    let (mut m, t0) = (full(), Instant::now());
    let fault = Input::Worker(Event::Faulted {
        cause: "release unconfirmed".into(),
    });
    script(
        &mut m,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
            (1, fault.clone(), vec![Effect::Terminate]),
            (2, fault, vec![]),
            (3, Exited(Exit::Code(3)), vec![]),
        ],
    );
    assert_eq!(
        message(&m.state(t0)),
        Some("worker faulted: release unconfirmed")
    );
    script(
        &mut m,
        t0,
        vec![
            (
                1_003,
                Tick,
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
            (
                1_004,
                Input::WorkerBroken("malformed".into()),
                vec![Effect::Terminate],
            ),
            (1_005, Exited(Exit::Code(1)), vec![]),
            (
                3_005,
                Tick,
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
            (3_006, Input::SpawnFailed("no such file".into()), vec![]),
        ],
    );
    let state = m.state(t0);
    assert!(matches!(full_state(&state).1, FullWorker::Retrying { .. }));
    assert_eq!(
        message(&state),
        Some("cannot start the worker: no such file")
    );
}

#[test]
fn an_exit_keeps_the_workers_last_failure() {
    let (mut m, t0) = (full(), Instant::now());
    script(
        &mut m,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
            (
                1,
                full_status(FullStatus {
                    failure: Some(WorkerFailure::other("output: refused")),
                    ..full_report(LightingTarget::ScreenSync)
                }),
                vec![],
            ),
            (2, Exited(Exit::Code(1)), vec![]),
        ],
    );
    assert_eq!(
        message(&m.state(t0)),
        Some("worker failed: output: refused")
    );
}

/// A slim worker that dies after a report: the exit is a failure with the
/// report's kind, except that a receiver wait or a failed receiver (no
/// reason for the worker to exit) is `other`.
#[test]
fn an_exit_keeps_an_actionable_kind_but_not_a_receiver_wait() {
    for (reported, published) in [
        (FailureKind::Receiver, FailureKind::Other),
        (FailureKind::Encoder, FailureKind::Encoder),
        (FailureKind::Version, FailureKind::Version),
        (FailureKind::Replaced, FailureKind::Replaced),
        (FailureKind::ReceiverFailed, FailureKind::Other),
        (FailureKind::Other, FailureKind::Other),
    ] {
        let mut m = slim(Preferences {
            autostart: true,
            ..slim_prefs()
        });
        let t0 = Instant::now();
        script(
            &mut m,
            t0,
            vec![
                (
                    0,
                    Pw(Active),
                    vec![spawn_slim(&receiver()), video(Video::Unknown)],
                ),
                (1, slim_failure(reported, "refused"), vec![]),
                (2, Exited(Exit::Signal(11)), vec![]),
            ],
        );
        let state = m.state(t0);
        assert_eq!(kind(&state), Some(published), "{reported:?}");
        assert_eq!(message(&state), Some("worker killed by signal 11: refused"));
    }
}

/// A worker that fails says why before it exits, also before its first
/// status; the cause wins over its last report, and the target it last
/// reported stays the intent.
#[test]
fn an_exit_names_why_the_worker_failed() {
    let (mut m, t0) = (full(), Instant::now());
    let failed = |cause: &str| {
        Input::Worker(Event::Failed {
            cause: cause.into(),
        })
    };
    script(
        &mut m,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
            (1, failed("cannot listen on 0.0.0.0:8787"), vec![]),
        ],
    );
    assert_eq!(message(&m.state(t0)), Some("cannot listen on 0.0.0.0:8787"));
    script(&mut m, t0, vec![(2, Exited(Exit::Code(1)), vec![])]);
    let state = m.state(t0);
    assert_eq!(
        message(&state),
        Some("worker failed: cannot listen on 0.0.0.0:8787")
    );
    assert_eq!(kind(&state), Some(FailureKind::Other));
    script(
        &mut m,
        t0,
        vec![
            (
                1_002,
                Tick,
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
            (
                1_003,
                full_status(FullStatus {
                    failure: Some(WorkerFailure::other("output: refused")),
                    ..full_report(LightingTarget::Effects)
                }),
                vec![],
            ),
            (1_004, failed("engine stopped"), vec![]),
            (1_005, Exited(Exit::Code(1)), vec![]),
        ],
    );
    let state = m.state(t0);
    assert_eq!(message(&state), Some("worker failed: engine stopped"));
    assert_eq!(full_state(&state).0, LightingTarget::Effects);
    // The next worker starts without the old cause.
    script(
        &mut m,
        t0,
        vec![
            (
                3_005,
                Tick,
                vec![spawn_full(LightingTarget::Effects), video(Video::Unknown)],
            ),
            (3_006, Exited(Exit::Code(1)), vec![]),
        ],
    );
    assert_eq!(message(&m.state(t0)), Some("worker failed"));
}

/// A status of the other package is a broken link, never guessed at: the
/// worker is stopped as failed and nothing of the report is kept.
#[test]
fn the_other_packages_status_breaks_the_link() {
    let (mut m, t0) = (full(), Instant::now());
    script(
        &mut m,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
            (
                1,
                slim_status(SlimStatus {
                    capturing: true,
                    ..SlimStatus::default()
                }),
                vec![Effect::Terminate],
            ),
            (2, Exited(Exit::Code(0)), vec![]),
        ],
    );
    let state = m.state(t0);
    assert!(matches!(full_state(&state).1, FullWorker::Retrying { .. }));
    assert!(
        message(&state).is_some_and(|text| text.contains("other package")),
        "{state:?}"
    );

    let mut m = slim(Preferences {
        autostart: true,
        ..slim_prefs()
    });
    script(
        &mut m,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_slim(&receiver()), video(Video::Unknown)],
            ),
            (
                1,
                status(LightingTarget::ScreenSync),
                vec![Effect::Terminate],
            ),
        ],
    );
    assert_eq!(slim_state(&m.state(t0)), (true, &SlimWorker::Stopping));
}

#[test]
fn shutdown_stops_and_finishes_from_every_phase() {
    let t0 = Instant::now();
    let mut idle = full();
    script(&mut idle, t0, vec![(0, Input::Shutdown, vec![])]);
    assert!(idle.finished());

    let mut running = full();
    script(
        &mut running,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
            (1, Input::Shutdown, vec![Effect::Terminate]),
            (2, Pw(Active), vec![]),
        ],
    );
    assert!(!running.finished());
    script(&mut running, t0, vec![(3, Exited(Exit::Code(0)), vec![])]);
    assert!(running.finished());

    let mut retrying = full();
    script(
        &mut retrying,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
            (1, Exited(Exit::Code(1)), vec![]),
            (2, Input::Shutdown, vec![]),
            (5_000, Tick, vec![]),
        ],
    );
    assert!(retrying.finished());
}

#[test]
fn context_changes_reset_only_a_running_worker() {
    let (mut m, t0) = (full(), Instant::now());
    script(
        &mut m,
        t0,
        vec![
            (0, Input::VideoContextChanged, vec![]),
            (
                0,
                Pw(Active),
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
            (
                1,
                Input::VideoContextChanged,
                vec![Effect::Send(Command::ResetVideo {})],
            ),
        ],
    );
}

#[test]
fn log_keeps_the_latest_lines_and_publish_follows_changes_only() {
    let (mut m, t0) = (full(), Instant::now());
    for line in 0..25 {
        m.handle(Input::WorkerLog(format!("line {line}")), t0);
    }
    let log = m.state(t0).log;
    assert_eq!(log.len(), LOG_LINES);
    assert_eq!(log[0], "line 5");
    assert!(!m.handle(Pw(Unknown), t0).contains(&Effect::Publish));
    assert!(m.handle(Pw(Asleep), t0).contains(&Effect::Publish));
    assert!(
        m.handle(Input::WorkerLog("x".into()), t0)
            .contains(&Effect::Publish)
    );
}

#[test]
fn published_state_reflects_the_worker() {
    let (mut m, t0) = (full(), Instant::now());
    m.handle(Pw(Active), t0);
    m.handle(
        full_status(FullStatus {
            capturing: true,
            fps: Some(30),
            target: LightingTarget::ScreenSync,
            portal: Some("http://10.0.0.2:8787/".into()),
            failure: None,
        }),
        t0,
    );
    let state = m.state(t0);
    assert_eq!(state.package.flavor(), crate::state::Flavor::Full);
    let json = serde_json::to_value(&state).unwrap();
    assert_eq!(
        json,
        json!({
            "flavor": "full",
            "target": "screen_sync",
            "worker": {
                "phase": "running",
                "capturing": true,
                "fps": 30,
                "portal": "http://10.0.0.2:8787/",
            },
            "preferences": { "autostart": false },
            "power": "active",
            "video": "unknown",
            "log": [],
        })
    );
    m.handle(Exited(Exit::Code(1)), t0);
    let state = m.state(t0);
    assert!(matches!(full_state(&state).1, FullWorker::Retrying { .. }));
    assert_eq!(full_state(&state).0, LightingTarget::ScreenSync);
}

#[test]
fn published_slim_state_has_only_its_own_fields() {
    let mut m = slim(Preferences {
        autostart: true,
        ..slim_prefs()
    });
    let t0 = Instant::now();
    m.handle(Pw(Active), t0);
    m.handle(
        slim_status(SlimStatus {
            capturing: true,
            link: SlimLink::Sending {
                encoding: SlimEncoding::Raw,
                fps: 25,
            },
        }),
        t0,
    );
    let json = serde_json::to_value(m.state(t0)).unwrap();
    assert_eq!(json["flavor"], "slim");
    assert_eq!(json["enabled"], true);
    assert_eq!(
        json["worker"],
        json!({
            "phase": "running",
            "capturing": true,
            "link": { "state": "sending", "encoding": "raw", "fps": 25 },
        })
    );
    let mut fields: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        [
            "enabled",
            "flavor",
            "log",
            "power",
            "preferences",
            "video",
            "worker"
        ]
    );
}

#[test]
fn only_a_running_worker_is_capturing() {
    let (mut m, t0) = (
        full_with(Preferences {
            autostart: true,
            ..Preferences::default()
        }),
        Instant::now(),
    );
    let capturing = full_status(FullStatus {
        capturing: true,
        portal: Some("http://10.0.0.2:8787/".into()),
        ..full_report(LightingTarget::ScreenSync)
    });
    script(
        &mut m,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![
                    spawn_full(LightingTarget::ScreenSync),
                    video(Video::Unknown),
                ],
            ),
            (1, capturing.clone(), vec![]),
        ],
    );
    assert!(full_capturing(&m.state(t0)));
    script(
        &mut m,
        t0,
        vec![
            (
                2,
                Target(LightingTarget::Off),
                vec![send_target(LightingTarget::Off)],
            ),
            (3, status(LightingTarget::Off), vec![]),
        ],
    );
    let state = m.state(t0);
    assert!(!full_capturing(&state));
    assert_eq!(full_state(&state).0, LightingTarget::Off);

    // A worker being stopped, or a late line from it, never shows capturing.
    script(
        &mut m,
        t0,
        vec![(
            4,
            Target(LightingTarget::ScreenSync),
            vec![send_target(LightingTarget::ScreenSync)],
        )],
    );
    script(&mut m, t0, vec![(5, capturing.clone(), vec![])]);
    assert!(full_capturing(&m.state(t0)));
    script(
        &mut m,
        t0,
        vec![
            (
                6,
                Pw(Asleep),
                vec![video(Video::Unavailable), Effect::Terminate],
            ),
            (7, capturing, vec![]),
        ],
    );
    // The portal still answers until the worker exits.
    assert_eq!(
        full_state(&m.state(t0)).1,
        &FullWorker::Stopping {
            portal: Some("http://10.0.0.2:8787/".into())
        }
    );
}

#[test]
fn failures_carry_their_kind() {
    let mut m = slim(Preferences {
        autostart: true,
        ..slim_prefs()
    });
    let t0 = Instant::now();
    script(
        &mut m,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_slim(&receiver()), video(Video::Unknown)],
            ),
            (
                1,
                slim_failure(FailureKind::Receiver, "waiting for the receiver: refused"),
                vec![],
            ),
        ],
    );
    let state = m.state(t0);
    assert_eq!(kind(&state), Some(FailureKind::Receiver));
    assert_eq!(
        serde_json::to_value(&state).unwrap()["failure"]["kind"],
        "receiver"
    );

    // Prysm of another capture stream version: its own kind, not a wait.
    script(
        &mut m,
        t0,
        vec![(
            1,
            slim_failure(
                FailureKind::Version,
                "waiting for the receiver: the capture stream versions differ",
            ),
            vec![],
        )],
    );
    let state = m.state(t0);
    assert_eq!(kind(&state), Some(FailureKind::Version));
    assert_eq!(
        serde_json::to_value(&state).unwrap()["failure"]["kind"],
        "version"
    );

    // The worker's last status names the encoder; the exit keeps that kind,
    // so the TV app still offers another encoding during the backoff.
    script(
        &mut m,
        t0,
        vec![
            (
                2,
                slim_failure(FailureKind::Encoder, "hardware H.264 cannot start"),
                vec![],
            ),
            (3, Exited(Exit::Code(1)), vec![]),
        ],
    );
    let state = m.state(t0);
    assert_eq!(
        message(&state),
        Some("worker failed: hardware H.264 cannot start")
    );
    assert_eq!(kind(&state), Some(FailureKind::Encoder));

    // The supervisor's own failures are `other`; a worker's keep their kind.
    // (A report without a kind is `other` already on the wire.)
    script(
        &mut m,
        t0,
        vec![
            (
                1_003,
                Tick,
                vec![spawn_slim(&receiver()), video(Video::Unknown)],
            ),
            (1_004, Input::SpawnFailed("no such file".into()), vec![]),
        ],
    );
    assert_eq!(kind(&m.state(t0)), Some(FailureKind::Other));
    let mut full = full();
    full.handle(Pw(Active), t0);
    full.handle(
        full_status(FullStatus {
            failure: Some(WorkerFailure::other("x")),
            ..full_report(LightingTarget::Off)
        }),
        t0,
    );
    assert_eq!(kind(&full.state(t0)), Some(FailureKind::Other));
}

/// A receiver that paused the stream (its lighting is off) is not a
/// failure: the state says paused, an old failure is cleared, and only a
/// running worker can be paused.
#[test]
fn a_paused_receiver_is_published_without_a_failure() {
    let mut m = slim(Preferences {
        autostart: true,
        ..slim_prefs()
    });
    let t0 = Instant::now();
    script(
        &mut m,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_slim(&receiver()), video(Video::Unknown)],
            ),
            (1, Exited(Exit::Code(1)), vec![]),
            (
                1_001,
                Tick,
                vec![spawn_slim(&receiver()), video(Video::Unknown)],
            ),
            (1_002, slim_link(SlimLink::Paused {}), vec![]),
        ],
    );
    let state = m.state(t0);
    assert_eq!(
        slim_state(&state).1,
        &SlimWorker::Running {
            capturing: false,
            link: SlimLink::Paused {},
        }
    );
    assert_eq!(state.failure, None);
    assert_eq!(
        serde_json::to_value(m.state(t0)).unwrap()["worker"]["link"],
        json!({ "state": "paused" })
    );
    script(
        &mut m,
        t0,
        vec![(
            1_003,
            Pw(Asleep),
            vec![video(Video::Unavailable), Effect::Terminate],
        )],
    );
    assert_eq!(
        slim_state(&m.state(t0)).1,
        &SlimWorker::Stopping,
        "a worker being stopped is not paused"
    );
}

#[test]
fn a_worker_that_is_up_again_clears_the_old_failure_before_a_healthy_minute() {
    let mut m = slim(Preferences {
        autostart: true,
        ..slim_prefs()
    });
    let t0 = Instant::now();
    script(
        &mut m,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_slim(&receiver()), video(Video::Unknown)],
            ),
            (1, Exited(Exit::Code(1)), vec![]),
            (
                1_001,
                Tick,
                vec![spawn_slim(&receiver()), video(Video::Unknown)],
            ),
            // Started but not yet streaming: the failure stays.
            (1_002, slim_status(SlimStatus::default()), vec![]),
        ],
    );
    assert_eq!(message(&m.state(t0)), Some("worker failed"));
    script(
        &mut m,
        t0,
        vec![(
            2_000,
            slim_status(SlimStatus {
                capturing: true,
                link: SlimLink::Sending {
                    encoding: SlimEncoding::H264Hardware,
                    fps: 30,
                },
            }),
            vec![],
        )],
    );
    assert_eq!(m.state(t0).failure, None);
    assert_eq!(kind(&m.state(t0)), None);
    // Backoff accounting is unchanged: still doubled until a healthy minute.
    script(
        &mut m,
        t0,
        vec![
            (3_000, Exited(Exit::Code(1)), vec![]),
            (4_999, Tick, vec![]),
            (
                5_000,
                Tick,
                vec![spawn_slim(&receiver()), video(Video::Unknown)],
            ),
        ],
    );

    // Full: a worker reporting without a failure is up.
    let mut full = full();
    script(
        &mut full,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
            (1, Exited(Exit::Code(1)), vec![]),
            (
                1_001,
                Tick,
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
        ],
    );
    assert!(full.state(t0).failure.is_some());
    script(
        &mut full,
        t0,
        vec![(1_100, status(LightingTarget::Off), vec![])],
    );
    assert_eq!(full.state(t0).failure, None);
}

#[test]
fn a_preferences_change_during_backoff_retries_at_once_with_a_fresh_delay() {
    let mut m = slim(Preferences {
        autostart: true,
        ..slim_prefs()
    });
    let t0 = Instant::now();
    let raw = other_receiver();
    let mut steps = vec![(
        0,
        Pw(Active),
        vec![spawn_slim(&receiver()), video(Video::Unknown)],
    )];
    // Fail often enough that the next delay is long.
    let mut now = 0;
    for delay in [1_000, 2_000, 4_000, 8_000] {
        steps.push((now + 1, Exited(Exit::Code(1)), vec![]));
        steps.push((
            now + 1 + delay,
            Tick,
            vec![spawn_slim(&receiver()), video(Video::Unknown)],
        ));
        now += 1 + delay;
    }
    steps.push((now + 1, Exited(Exit::Code(1)), vec![]));
    script(&mut m, t0, steps);
    assert!(matches!(
        slim_state(&m.state(t0)).1,
        SlimWorker::Retrying { .. }
    ));

    // The new settings start now, not after the 16 s backoff.
    script(
        &mut m,
        t0,
        vec![(
            now + 2,
            Input::Preferences(Preferences {
                autostart: true,
                instance: Some(raw.clone()),
            }),
            vec![spawn_slim(&raw), video(Video::Unknown)],
        )],
    );
    // And a failure of the new attempt waits only the first delay.
    script(
        &mut m,
        t0,
        vec![
            (now + 3, Exited(Exit::Code(1)), vec![]),
            (now + 1_002, Tick, vec![]),
            (
                now + 1_003,
                Tick,
                vec![spawn_slim(&raw), video(Video::Unknown)],
            ),
        ],
    );
}

#[test]
fn a_preferences_change_while_a_failed_worker_stops_restarts_on_reaping() {
    let mut m = slim(Preferences {
        autostart: true,
        ..slim_prefs()
    });
    let t0 = Instant::now();
    let raw = other_receiver();
    script(
        &mut m,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_slim(&receiver()), video(Video::Unknown)],
            ),
            (
                1,
                Input::WorkerBroken("malformed".into()),
                vec![Effect::Terminate],
            ),
            (
                2,
                Input::Preferences(Preferences {
                    autostart: true,
                    instance: Some(raw.clone()),
                }),
                vec![],
            ),
            (
                3,
                Exited(Exit::Code(1)),
                vec![spawn_slim(&raw), video(Video::Unknown)],
            ),
        ],
    );
}

/// A stop the user or the TV asked for that ends in a fault, a signal or a
/// forced kill is a failure: the worker could not release what it held.
#[test]
fn a_requested_stop_that_ends_badly_is_a_failure() {
    let t0 = Instant::now();
    // Slim: the worker says why, then exits faulted.
    let mut m = slim(Preferences {
        autostart: true,
        ..slim_prefs()
    });
    script(
        &mut m,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_slim(&receiver()), video(Video::Unknown)],
            ),
            (1, Enabled(false), vec![Effect::Terminate]),
            (
                2,
                Input::Worker(Event::Failed {
                    cause: "release unconfirmed".into(),
                }),
                vec![],
            ),
            (3, Exited(Exit::Code(exit::FAULTED.into())), vec![]),
            (4, Tick, vec![]),
        ],
    );
    let state = m.state(t0);
    assert_eq!(slim_state(&state), (false, &SlimWorker::Stopped));
    assert_eq!(message(&state), Some("worker faulted: release unconfirmed"));
    // Full: the worker's fault while it stops.
    let mut m = full();
    script(
        &mut m,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
            (
                1,
                Pw(Asleep),
                vec![video(Video::Unavailable), Effect::Terminate],
            ),
            (
                2,
                Input::Worker(Event::Faulted {
                    cause: "release unconfirmed".into(),
                }),
                vec![],
            ),
            (3, Exited(Exit::Code(exit::FAULTED.into())), vec![]),
        ],
    );
    assert_eq!(
        message(&m.state(t0)),
        Some("worker faulted: release unconfirmed")
    );
    // A signal, and a worker that had to be killed.
    for (kill, exit, shown) in [
        (false, Exit::Signal(11), "worker killed by signal 11"),
        (true, Exit::Signal(9), "worker did not stop within 8 s"),
    ] {
        let mut m = full();
        let mut steps = vec![
            (
                0,
                Pw(Active),
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
            (
                1,
                Pw(Asleep),
                vec![video(Video::Unavailable), Effect::Terminate],
            ),
        ];
        if kill {
            steps.push((8_001, Tick, vec![Effect::Kill]));
        }
        steps.push((8_002, Exited(exit), vec![]));
        script(&mut m, t0, steps);
        let state = m.state(t0);
        assert_eq!(full_state(&state).1, &FullWorker::Stopped);
        assert!(
            message(&state).is_some_and(|text| text.starts_with(shown)),
            "{state:?}"
        );
    }
    // A clean exit is no failure.
    let mut m = full();
    script(
        &mut m,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
            (
                1,
                Pw(Asleep),
                vec![video(Video::Unavailable), Effect::Terminate],
            ),
            (2, Exited(Exit::Code(0)), vec![]),
        ],
    );
    assert_eq!(m.state(t0).failure, None);
}

/// A failure is shown until the user or the TV ends the worker: then the
/// published `Stopped` or the next worker carries no old failure.
/// Off sets the full worker's lighting; the worker stays wanted, so its
/// failure stays shown while it retries.
#[test]
fn turning_lighting_off_keeps_a_retrying_full_workers_failure() {
    let (mut m, t0) = (full(), Instant::now());
    script(
        &mut m,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
            (1, Exited(Exit::Code(1)), vec![]),
            (2, Enabled(false), vec![]),
        ],
    );
    assert_eq!(message(&m.state(t0)), Some("worker failed"));
}

#[test]
fn a_requested_stop_clears_the_failure_in_every_phase() {
    let t0 = Instant::now();
    let retrying = || {
        let mut m = slim(Preferences {
            autostart: true,
            ..slim_prefs()
        });
        script(
            &mut m,
            t0,
            vec![
                (
                    0,
                    Pw(Active),
                    vec![spawn_slim(&receiver()), video(Video::Unknown)],
                ),
                (1, Exited(Exit::Code(1)), vec![]),
            ],
        );
        assert_eq!(message(&m.state(t0)), Some("worker failed"));
        m
    };
    for input in [Enabled(false), Pw(Asleep)] {
        let mut m = retrying();
        script(&mut m, t0, vec![(2, input.clone(), vec![])]);
        let state = m.state(t0);
        assert_eq!(slim_state(&state).1, &SlimWorker::Stopped, "{input:?}");
        assert_eq!(state.failure, None, "{input:?}");
    }
    // A new address: the new worker starts without the old one's failure.
    let mut m = retrying();
    let other = other_receiver();
    script(
        &mut m,
        t0,
        vec![(
            2,
            Input::Preferences(Preferences {
                autostart: true,
                instance: Some(other.clone()),
            }),
            vec![spawn_slim(&other), video(Video::Unknown)],
        )],
    );
    assert_eq!(m.state(t0).failure, None);
    // A failed worker being stopped, then disabled: stopped, no failure.
    let mut m = slim(Preferences {
        autostart: true,
        ..slim_prefs()
    });
    script(
        &mut m,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_slim(&receiver()), video(Video::Unknown)],
            ),
            (
                1,
                Input::WorkerBroken("malformed".into()),
                vec![Effect::Terminate],
            ),
            (2, Enabled(false), vec![]),
            (3, Exited(Exit::Code(1)), vec![]),
        ],
    );
    let state = m.state(t0);
    assert_eq!(slim_state(&state).1, &SlimWorker::Stopped);
    assert_eq!(state.failure, None);
}

/// A slim worker that reached the receiver is up, frames or not: the last
/// worker's failure is no longer current.
#[test]
fn a_connected_slim_worker_without_frames_is_up() {
    let mut m = slim(Preferences {
        autostart: true,
        ..slim_prefs()
    });
    let t0 = Instant::now();
    script(
        &mut m,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_slim(&receiver()), video(Video::Unknown)],
            ),
            (1, Exited(Exit::Code(1)), vec![]),
            (
                1_001,
                Tick,
                vec![spawn_slim(&receiver()), video(Video::Unknown)],
            ),
            (
                1_002,
                slim_link(SlimLink::Sending {
                    encoding: SlimEncoding::Raw,
                    fps: 0,
                }),
                vec![],
            ),
        ],
    );
    assert_eq!(m.state(t0).failure, None);
}

/// While a failed worker stops, the supervisor's reason is the failure, not
/// the worker's last report.
#[test]
fn a_failed_stop_shows_why_it_was_stopped() {
    let mut m = slim(Preferences {
        autostart: true,
        ..slim_prefs()
    });
    let t0 = Instant::now();
    script(
        &mut m,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_slim(&receiver()), video(Video::Unknown)],
            ),
            (
                1,
                slim_failure(FailureKind::Receiver, "waiting for the receiver: refused"),
                vec![],
            ),
            (
                2,
                Input::WorkerBroken("malformed".into()),
                vec![Effect::Terminate],
            ),
        ],
    );
    let state = m.state(t0);
    assert_eq!(message(&state), Some("worker link broke: malformed"));
    assert_eq!(kind(&state), Some(FailureKind::Other));
}

/// A worker that is not reaped [`KILL_AFTER`] after its kill is let go, so
/// a shutdown still finishes.
#[test]
fn a_worker_never_reaped_after_its_kill_is_let_go() {
    let (mut m, t0) = (full(), Instant::now());
    script(
        &mut m,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
            (1, Input::Shutdown, vec![Effect::Terminate]),
            (8_001, Tick, vec![Effect::Kill]),
            (16_000, Tick, vec![]),
            (16_001, Tick, vec![Effect::Abandon]),
        ],
    );
    assert!(m.finished());
}

/// A phase without its times.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Is {
    Idle,
    Running,
    Stopping(Sent, AfterExit),
    Retrying,
}

fn is(phase: Phase) -> Is {
    match phase {
        Phase::Idle => Is::Idle,
        Phase::Running { .. } => Is::Running,
        Phase::Stopping { sent, then, .. } => Is::Stopping(sent, then),
        Phase::Retrying { .. } => Is::Retrying,
    }
}

/// A slim machine in a phase: built by `steps` (from a worker started at 0
/// unless disabled), then sent each input at `now` (or, for the timer, at
/// `due`).
struct From {
    name: &'static str,
    enabled: bool,
    steps: Vec<(u64, Input, Vec<Effect>)>,
    now: u64,
    due: u64,
}

impl From {
    fn build(&self, t0: Instant) -> Machine {
        let mut m = slim(Preferences {
            autostart: self.enabled,
            ..slim_prefs()
        });
        let start = if self.enabled {
            vec![spawn_slim(&receiver()), video(Video::Unknown)]
        } else {
            vec![]
        };
        let mut steps = vec![(0, Pw(Active), start)];
        steps.extend(self.steps.iter().cloned());
        script(&mut m, t0, steps);
        m
    }
}

/// Every phase x every input: the effects, the next phase and the published
/// failure. Each row is one input; its cells are the phases in the order of
/// `froms`. The idle machine shows the failure of a worker that faulted as
/// it stopped.
#[test]
fn every_phase_takes_every_input() {
    use AfterExit::{Idle as ThenIdle, Retry};
    use Sent::{Kill, Terminate};
    let other = other_receiver();
    let broke = || {
        (
            500,
            Input::WorkerBroken("x".into()),
            vec![Effect::Terminate],
        )
    };
    let disable = || (500, Enabled(false), vec![Effect::Terminate]);
    let kill = || (8_500, Tick, vec![Effect::Kill]);
    let froms = [
        From {
            name: "idle",
            enabled: true,
            steps: vec![
                (100, Enabled(false), vec![Effect::Terminate]),
                (200, Exited(Exit::Code(exit::FAULTED.into())), vec![]),
            ],
            now: 1_000,
            due: 1_000,
        },
        From {
            name: "running",
            enabled: true,
            steps: vec![],
            now: 1_000,
            due: 15_000,
        },
        From {
            name: "stopping as asked",
            enabled: true,
            steps: vec![disable()],
            now: 1_000,
            due: 8_500,
        },
        From {
            name: "stopping as failed",
            enabled: true,
            steps: vec![broke()],
            now: 1_000,
            due: 8_500,
        },
        From {
            name: "killed as asked",
            enabled: true,
            steps: vec![disable(), kill()],
            now: 9_000,
            due: 16_500,
        },
        From {
            name: "killed as failed",
            enabled: true,
            steps: vec![broke(), kill()],
            now: 9_000,
            due: 16_500,
        },
        From {
            name: "killed for new settings",
            enabled: true,
            steps: vec![
                (
                    500,
                    Input::Preferences(Preferences {
                        autostart: true,
                        instance: Some(other.clone()),
                    }),
                    vec![Effect::Terminate],
                ),
                kill(),
            ],
            now: 9_000,
            due: 16_500,
        },
        From {
            name: "retrying",
            enabled: true,
            steps: vec![(500, Exited(Exit::Code(1)), vec![])],
            now: 1_000,
            due: 1_500,
        },
    ];
    let (idle, running, retrying) = (Is::Idle, Is::Running, Is::Retrying);
    let (asked, failing) = (
        Is::Stopping(Terminate, ThenIdle),
        Is::Stopping(Terminate, Retry),
    );
    let (killed, killed_failing) = (Is::Stopping(Kill, ThenIdle), Is::Stopping(Kill, Retry));
    let (broken, failed) = (Some("worker link broke: x"), Some("worker failed"));
    let faulted = Some("worker faulted");
    let not_stopped = Some("worker did not stop within 8 s");
    let none = Vec::new;
    let terminate = || vec![Effect::Terminate];
    let start = |at: &PrysmAddress| vec![spawn_slim(at), video(Video::Unknown)];
    type Cell = (Vec<Effect>, Is, Option<&'static str>);
    // Nothing changes in any phase but running, which gets `running`.
    let only_running = |running: Cell| -> [Cell; 8] {
        [
            (none(), idle, faulted),
            running,
            (none(), asked, None),
            (none(), failing, broken),
            (none(), killed, None),
            (none(), killed_failing, broken),
            (none(), killed, None),
            (none(), retrying, failed),
        ]
    };
    let unchanged = || only_running((none(), running, None));
    // Turned off: a requested stop in every phase, clearing the failure.
    let off = |running: Vec<Effect>| -> [Cell; 8] {
        [
            (none(), idle, None),
            (running, asked, None),
            (none(), asked, None),
            (none(), asked, None),
            (none(), killed, None),
            (none(), killed, None),
            (none(), killed, None),
            (none(), idle, None),
        ]
    };
    // An exit: the last cell respawns at once for the new settings.
    let exited = |running: &'static str, asked: Option<&'static str>| -> [Cell; 8] {
        [
            (none(), idle, faulted),
            (none(), retrying, Some(running)),
            (none(), idle, asked),
            (none(), retrying, broken),
            (none(), idle, not_stopped),
            (none(), retrying, broken),
            (start(&other), Is::Running, not_stopped),
            (none(), retrying, failed),
        ]
    };
    let worker = |event| Input::Worker(event);
    let sending = worker(Event::Status(WorkerStatus::Slim(SlimStatus {
        capturing: true,
        link: SlimLink::Sending {
            encoding: SlimEncoding::Raw,
            fps: 30,
        },
    })));
    let cause = || "c".to_owned();
    let last_word = |running: Cell| -> [Cell; 8] {
        [
            (none(), idle, faulted),
            running,
            (none(), asked, Some("c")),
            (none(), failing, broken),
            (none(), killed, Some("c")),
            (none(), killed_failing, broken),
            (none(), killed, Some("c")),
            (none(), retrying, failed),
        ]
    };
    let table: Vec<(Input, bool, [Cell; 8])> = vec![
        (
            Pw(Asleep),
            false,
            off(vec![video(Video::Unavailable), Effect::Terminate]),
        ),
        (
            V(Video::Available),
            false,
            only_running((vec![video(Video::Available)], running, None)),
        ),
        (
            Input::VideoContextChanged,
            false,
            only_running((vec![Effect::Send(Command::ResetVideo {})], running, None)),
        ),
        (
            Input::Preferences(Preferences {
                autostart: false,
                instance: Some(other.clone()),
            }),
            false,
            [
                (none(), idle, None),
                (terminate(), asked, None),
                (none(), asked, None),
                (none(), asked, None),
                (none(), killed, None),
                (none(), killed, None),
                // The same address: nothing to restart.
                (none(), killed, None),
                (start(&other), running, None),
            ],
        ),
        (Enabled(false), false, off(terminate())),
        // The full package's method: refused by the service, ignored here.
        (Target(LightingTarget::Off), false, unchanged()),
        (Target(LightingTarget::ScreenSync), false, unchanged()),
        (Target(LightingTarget::Effects), false, unchanged()),
        (
            Enabled(true),
            false,
            [
                (start(&receiver()), running, faulted),
                (none(), running, None),
                (none(), asked, None),
                (none(), failing, broken),
                (none(), killed, None),
                (none(), killed_failing, broken),
                (none(), killed, None),
                (none(), retrying, failed),
            ],
        ),
        (sending, false, unchanged()),
        (
            worker(Event::Failed { cause: cause() }),
            false,
            last_word((none(), running, Some("c"))),
        ),
        (
            worker(Event::Faulted { cause: cause() }),
            false,
            last_word((terminate(), failing, Some("worker faulted: c"))),
        ),
        (Input::WorkerLog("l".into()), false, unchanged()),
        (
            Input::WorkerBroken("y".into()),
            false,
            only_running((terminate(), failing, Some("worker link broke: y"))),
        ),
        (
            Input::SpawnFailed("s".into()),
            false,
            only_running((none(), retrying, Some("cannot start the worker: s"))),
        ),
        (Exited(Exit::Code(0)), false, exited("worker stopped", None)),
        (
            Exited(Exit::Code(exit::FAULTED.into())),
            false,
            exited("worker faulted", faulted),
        ),
        // SIGTERM before the worker's handlers are in: a clean stop.
        (
            Exited(Exit::Signal(15)),
            false,
            exited("worker killed by signal 15", None),
        ),
        (
            Exited(Exit::Signal(11)),
            false,
            exited(
                "worker killed by signal 11",
                Some("worker killed by signal 11"),
            ),
        ),
        (Tick, false, unchanged()),
        (
            Tick,
            true,
            [
                (none(), idle, faulted),
                (terminate(), failing, Some("worker sent nothing for 15 s")),
                (vec![Effect::Kill], killed, None),
                (vec![Effect::Kill], killed_failing, broken),
                (vec![Effect::Abandon], idle, not_stopped),
                (vec![Effect::Abandon], retrying, broken),
                // The abandoned worker may still hold the capture.
                (vec![Effect::Abandon], retrying, not_stopped),
                (start(&receiver()), running, failed),
            ],
        ),
        (Input::Shutdown, false, off(terminate())),
    ];
    let t0 = Instant::now();
    for (input, due, cells) in table {
        for (from, (effects, next, failure)) in froms.iter().zip(cells) {
            let mut m = from.build(t0);
            let at = if due { from.due } else { from.now };
            let shown = format!("{} x {input:?} at {at} ms", from.name);
            let done: Vec<_> = m
                .handle(input.clone(), t0 + Duration::from_millis(at))
                .into_iter()
                .filter(|effect| *effect != Effect::Publish)
                .collect();
            assert_eq!(done, effects, "{shown}");
            assert_eq!(is(m.phase), next, "{shown}");
            assert_eq!(message(&m.state(t0)), failure, "{shown}");
        }
    }
}

/// The full package's lighting inputs from each target, with the worker
/// asleep (idle) or running: a target is kept and sent to a running worker,
/// and the slim package's `Enabled` changes nothing.
#[test]
fn every_full_target_takes_every_lighting_input() {
    use LightingTarget::{Effects, Off, ScreenSync};
    let inputs = [
        Target(Off),
        Target(ScreenSync),
        Target(Effects),
        Enabled(false),
        Enabled(true),
    ];
    let t0 = Instant::now();
    for from in LightingTarget::ALL {
        for awake in [false, true] {
            for input in inputs.clone() {
                let mut m = full();
                let mut steps = vec![(0, Target(from), vec![])];
                if awake {
                    steps.push((0, Pw(Active), vec![spawn_full(from), video(Video::Unknown)]));
                }
                script(&mut m, t0, steps);
                let (effects, intent) = match input {
                    Target(target) if awake => (vec![send_target(target)], target),
                    Target(target) => (vec![], target),
                    _ => (vec![], from),
                };
                let shown = format!("{from:?}, awake {awake}, {input:?}");
                let done: Vec<_> = m
                    .handle(input, t0 + Duration::from_millis(1))
                    .into_iter()
                    .filter(|effect| *effect != Effect::Publish)
                    .collect();
                assert_eq!(done, effects, "{shown}");
                assert_eq!(m.intent(), intent, "{shown}");
                assert_eq!(full_state(&m.state(t0)).0, intent, "{shown}");
            }
        }
    }
}
