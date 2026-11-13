//! Every machine rule, as scripted inputs at explicit times.
use super::*;
use crate::state::Phase as P;
use prysm_webos_link::{FailureKind, PrysmAddress, SlimEncoding, WorkerFailure};
use serde_json::Value;

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

fn spawn_full(lighting: LightingTarget) -> Effect {
    Effect::Spawn(
        FullArgs {
            settings: "/s.json".into(),
            listen: "0.0.0.0:8787".parse().unwrap(),
            lighting,
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

fn lighting(target: LightingTarget) -> Effect {
    Effect::Send(Command::Lighting { target })
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

fn status(target: Option<LightingTarget>) -> Input {
    Input::Worker(Event::Status(WorkerStatus {
        target,
        ..WorkerStatus::default()
    }))
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
use Input::{Enabled, Power as Pw, Tick, Video as V, WorkerExited as Exited};

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
    assert_eq!(m.state(t).worker, P::Running);
}

#[test]
fn full_autostart_starts_processed_and_enable_maps_to_lighting() {
    let mut m = full_with(Preferences {
        autostart: true,
        ..Preferences::default()
    });
    let t = Instant::now();
    assert!(m.state(t).enabled);
    script(
        &mut m,
        t,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_full(LightingTarget::Processed), video(Video::Unknown)],
            ),
            (10, Enabled(false), vec![lighting(LightingTarget::Off)]),
            // An older report within the echo window does not undo it.
            (20, status(Some(LightingTarget::Processed)), vec![]),
        ],
    );
    assert!(!m.state(t).enabled);
    assert_eq!(m.state(t).lighting, Some(LightingTarget::Off));
    script(
        &mut m,
        t,
        vec![
            (30, status(Some(LightingTarget::Off)), vec![]),
            // The portal turned device-native lighting on.
            (40, status(Some(LightingTarget::DeviceNative)), vec![]),
        ],
    );
    assert!(m.state(t).enabled);
    assert_eq!(m.state(t).lighting, Some(LightingTarget::DeviceNative));
    script(
        &mut m,
        t,
        vec![
            // Enabling keeps a non-off target.
            (
                50,
                Enabled(true),
                vec![lighting(LightingTarget::DeviceNative)],
            ),
            (60, Enabled(false), vec![lighting(LightingTarget::Off)]),
            (2059, status(Some(LightingTarget::Processed)), vec![]),
        ],
    );
    assert_eq!(m.state(t).lighting, Some(LightingTarget::Off));
    // After the echo window the worker's report wins again.
    script(
        &mut m,
        t,
        vec![(2060, status(Some(LightingTarget::Processed)), vec![])],
    );
    assert_eq!(m.state(t).lighting, Some(LightingTarget::Processed));
}

#[test]
fn full_remembers_the_target_for_the_next_worker() {
    let (mut m, t) = (full(), Instant::now());
    script(
        &mut m,
        t,
        vec![
            (0, Enabled(true), vec![]),
            (
                0,
                Pw(Active),
                vec![spawn_full(LightingTarget::Processed), video(Video::Unknown)],
            ),
            (5, status(Some(LightingTarget::DeviceNative)), vec![]),
            (
                6,
                Pw(Asleep),
                vec![video(Video::Unavailable), Effect::Terminate],
            ),
            (7, Exited(Exit::Code(0)), vec![]),
            (
                8,
                Pw(Active),
                vec![
                    spawn_full(LightingTarget::DeviceNative),
                    video(Video::Unknown),
                ],
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
    assert_eq!(m.state(t).worker, P::Stopped);
    assert_eq!(m.state(t).lighting, None);
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
    let at = |ms| m.state(t0 + Duration::from_millis(ms)).worker;
    assert_eq!(at(3_000), P::Retrying { seconds: 4 });
    assert_eq!(at(3_001), P::Retrying { seconds: 4 });
    assert_eq!(at(4_000), P::Retrying { seconds: 3 });
    assert_eq!(at(6_999), P::Retrying { seconds: 1 });
    assert_eq!(
        serde_json::to_value(m.state(t0 + Duration::from_millis(3_500))).unwrap()["worker"],
        serde_json::json!({ "phase": "retrying", "seconds": 4 })
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
        script(&mut m, t0, vec![(at, status(None), vec![])]);
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
            (10_000, status(None), vec![]),
            (24_999, Tick, vec![]),
            (25_000, Tick, vec![Effect::Terminate]),
            // A late line from the stopping worker changes nothing.
            (26_000, status(None), vec![]),
            (32_999, Tick, vec![]),
            (33_000, Tick, vec![Effect::Kill]),
            (40_000, Tick, vec![]),
            (40_001, Exited(Exit::Signal(9)), vec![]),
        ],
    );
    let state = m.state(t0);
    assert!(matches!(state.worker, P::Retrying { .. }));
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
    assert!(matches!(state.worker, P::Retrying { .. }));
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
                Input::Worker(Event::Status(WorkerStatus {
                    failure: Some(WorkerFailure::other("cannot listen on 0.0.0.0:8787")),
                    ..WorkerStatus::default()
                })),
                vec![],
            ),
            (2, Exited(Exit::Code(1)), vec![]),
        ],
    );
    assert_eq!(
        message(&m.state(t0)),
        Some("worker failed: cannot listen on 0.0.0.0:8787")
    );
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
        Input::Worker(Event::Status(WorkerStatus {
            capturing: true,
            fps: Some(30),
            target: Some(LightingTarget::Processed),
            portal: Some("http://10.0.0.2:8787/".into()),
            failure: None,
            encoding: None,
            paused: false,
        })),
        t0,
    );
    let state = m.state(t0);
    assert_eq!(state.flavor, crate::state::Flavor::Full);
    assert!(state.capturing && state.enabled);
    assert_eq!(state.portal.as_deref(), Some("http://10.0.0.2:8787/"));
    let json = serde_json::to_value(&state).unwrap();
    assert_eq!(json["worker"], serde_json::json!({ "phase": "running" }));
    assert_eq!(json["power"], "active");
    assert_eq!(json["lighting"], "processed");
    assert_eq!(json["failure"], Value::Null);
    assert_eq!(json["fps"], 30);
    m.handle(Exited(Exit::Code(1)), t0);
    let state = m.state(t0);
    assert!(!state.capturing);
    assert_eq!((state.portal, state.fps), (None, None));
}

fn reported(status: WorkerStatus) -> Input {
    Input::Worker(Event::Status(status))
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
    let capturing = WorkerStatus {
        capturing: true,
        target: Some(LightingTarget::Processed),
        ..WorkerStatus::default()
    };
    script(
        &mut m,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_full(LightingTarget::Processed), video(Video::Unknown)],
            ),
            (1, reported(capturing.clone()), vec![]),
        ],
    );
    assert!(m.state(t0).capturing);
    script(
        &mut m,
        t0,
        vec![
            (2, Enabled(false), vec![lighting(LightingTarget::Off)]),
            (
                3,
                reported(WorkerStatus {
                    capturing: false,
                    target: Some(LightingTarget::Off),
                    ..WorkerStatus::default()
                }),
                vec![],
            ),
        ],
    );
    let state = m.state(t0);
    assert!(!state.capturing && !state.enabled);

    // A worker being stopped, or a late line from it, never shows capturing.
    script(
        &mut m,
        t0,
        vec![(4, Enabled(true), vec![lighting(LightingTarget::Processed)])],
    );
    script(&mut m, t0, vec![(5, reported(capturing.clone()), vec![])]);
    assert!(m.state(t0).capturing);
    script(
        &mut m,
        t0,
        vec![
            (
                6,
                Pw(Asleep),
                vec![video(Video::Unavailable), Effect::Terminate],
            ),
            (7, reported(capturing), vec![]),
        ],
    );
    let state = m.state(t0);
    assert_eq!(state.worker, P::Stopping);
    assert!(!state.capturing);
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
                reported(WorkerStatus {
                    failure: Some(WorkerFailure::new(
                        FailureKind::Receiver,
                        "waiting for the receiver: refused",
                    )),
                    ..WorkerStatus::default()
                }),
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

    // The worker's final status names the encoder; the exit keeps the kind.
    script(
        &mut m,
        t0,
        vec![
            (
                2,
                reported(WorkerStatus {
                    failure: Some(WorkerFailure::new(
                        FailureKind::Encoder,
                        "hardware H.264 cannot start",
                    )),
                    ..WorkerStatus::default()
                }),
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
        reported(WorkerStatus {
            failure: Some(WorkerFailure::other("x")),
            ..WorkerStatus::default()
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
            (
                1_002,
                reported(WorkerStatus {
                    paused: true,
                    ..WorkerStatus::default()
                }),
                vec![],
            ),
        ],
    );
    let state = m.state(t0);
    assert!(state.paused && !state.capturing);
    assert_eq!(state.failure, None);
    assert_eq!(serde_json::to_value(m.state(t0)).unwrap()["paused"], true);
    script(
        &mut m,
        t0,
        vec![(
            1_003,
            Pw(Asleep),
            vec![video(Video::Unavailable), Effect::Terminate],
        )],
    );
    assert!(!m.state(t0).paused, "a worker being stopped is not paused");
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
            (1_002, reported(WorkerStatus::default()), vec![]),
        ],
    );
    assert_eq!(message(&m.state(t0)), Some("worker failed"));
    script(
        &mut m,
        t0,
        vec![(
            2_000,
            reported(WorkerStatus {
                capturing: true,
                encoding: Some(SlimEncoding::H264Hardware),
                ..WorkerStatus::default()
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
        vec![(1_100, status(Some(LightingTarget::Off)), vec![])],
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
    assert!(matches!(m.state(t0).worker, P::Retrying { .. }));

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
