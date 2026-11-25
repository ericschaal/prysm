//! Every machine rule, as scripted inputs at explicit times.
use super::*;

mod restart;
mod table;

use crate::state::Asked;
use prysm_ports::StreamEncoding;
use prysm_webos_link::{Escalation, Failure, Fix, FullStatus, PrysmAddress, SlimLink, SlimStatus};
use serde_json::json;
use std::time::Duration;

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
        prysm_address: Some(receiver()),
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

fn send_target(target: LightingTarget, sequence: u64) -> Effect {
    Effect::Send(Command::Target { target, sequence })
}

/// The fix most failures ask for.
const TRY_AGAIN: Asked = Asked::Ask(Fix::TryAgain);

/// What the published state asks of the person, and why.
fn asked(state: &State) -> Option<(Asked, &str)> {
    state
        .package
        .problem()
        .map(|problem| (problem.asked, problem.message.as_str()))
}

/// A full worker's report of `target`: no frames, no portal, no failure,
/// no target command applied.
fn full_report(target: LightingTarget) -> FullStatus {
    FullStatus {
        capturing: false,
        fps: None,
        target,
        lights: None,
        applied: 0,
        portal: None,
        failure: None,
    }
}

/// A worker's failure: `escalation` and `message`.
fn failure(escalation: Escalation, message: &str) -> Failure {
    Failure {
        escalation,
        message: message.into(),
    }
}

/// A worker's last word: it fails with `escalation`, for `message`.
fn failed_with(escalation: Escalation, message: &str) -> Input {
    Input::Worker(Event::Failed(failure(escalation, message)))
}

fn reported(status: WorkerStatus) -> Input {
    Input::Worker(Event::Status(status))
}

/// A full worker reports `target` and nothing else.
fn status(target: LightingTarget) -> Input {
    full_status(full_report(target))
}

/// A full worker reports `target`, having applied target command `applied`.
fn status_after(target: LightingTarget, applied: u64) -> Input {
    full_status(FullStatus {
        applied,
        ..full_report(target)
    })
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
        full_state(&m.state()).1,
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
    assert_eq!(full_state(&m.state()).0, LightingTarget::ScreenSync);
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
                vec![send_target(LightingTarget::Off, 1)],
            ),
            // A report sent before the worker read the command, however
            // late it comes, does not undo it.
            (20, status(LightingTarget::ScreenSync), vec![]),
            (60_000, status(LightingTarget::ScreenSync), vec![]),
        ],
    );
    assert_eq!(full_state(&m.state()).0, LightingTarget::Off);
    script(
        &mut m,
        t,
        vec![
            (60_010, status_after(LightingTarget::Off, 1), vec![]),
            // The portal turned the lights' own effects on.
            (60_020, status_after(LightingTarget::Effects, 1), vec![]),
        ],
    );
    assert_eq!(full_state(&m.state()).0, LightingTarget::Effects);
    script(
        &mut m,
        t,
        vec![
            // The app's targets are sent as chosen, each with the next number.
            (
                60_030,
                Target(LightingTarget::ScreenSync),
                vec![send_target(LightingTarget::ScreenSync, 2)],
            ),
            (
                60_040,
                Target(LightingTarget::Off),
                vec![send_target(LightingTarget::Off, 3)],
            ),
            (60_050, status_after(LightingTarget::ScreenSync, 2), vec![]),
        ],
    );
    assert_eq!(full_state(&m.state()).0, LightingTarget::Off);
    // Once the worker applied the last one, its report wins again.
    script(
        &mut m,
        t,
        vec![(60_060, status_after(LightingTarget::ScreenSync, 3), vec![])],
    );
    assert_eq!(full_state(&m.state()).0, LightingTarget::ScreenSync);
}

/// A new worker starts with the target in its arguments and counts target
/// commands from 1 again: its reports count from the start.
#[test]
fn a_new_worker_counts_target_commands_afresh() {
    let (mut m, t) = (full(), Instant::now());
    script(
        &mut m,
        t,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
            (
                1,
                Target(LightingTarget::ScreenSync),
                vec![send_target(LightingTarget::ScreenSync, 1)],
            ),
            (
                2,
                Target(LightingTarget::Effects),
                vec![send_target(LightingTarget::Effects, 2)],
            ),
            // Restarted at once with the target it last had.
            (
                3,
                Exited(Exit::Code(1)),
                vec![spawn_full(LightingTarget::Effects), video(Video::Unknown)],
            ),
            // The portal on the new worker turned the lights off.
            (1004, status(LightingTarget::Off), vec![]),
        ],
    );
    assert_eq!(full_state(&m.state()).0, LightingTarget::Off);
    script(
        &mut m,
        t,
        vec![(
            1005,
            Target(LightingTarget::ScreenSync),
            vec![send_target(LightingTarget::ScreenSync, 1)],
        )],
    );
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
    assert_eq!(slim_state(&m.state()), (false, &SlimWorker::Stopped));
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
                    prysm_address: Some(fast.clone()),
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
                    prysm_address: Some(fast.clone()),
                    autostart: false,
                }),
                vec![],
            ),
        ],
    );
    assert_eq!(m.state().package.problem(), None);
}

/// Silence is a hang: TERM, one KILL, and on its exit the restart, at once
/// and shown only in the tail.
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
            (
                40_001,
                Exited(Exit::Signal(9)),
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
        ],
    );
    let state = m.state();
    assert_eq!(asked(&state), None);
    assert_eq!(
        state.log,
        ["restarting the worker: worker sent nothing for 15 s"]
    );
}

/// A status of the other package is a broken link, never guessed at: the
/// worker is stopped, nothing of the report is kept, and the person is
/// asked.
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
    let state = m.state();
    assert!(
        matches!(full_state(&state).1, FullWorker::Asking { .. }),
        "{state:?}"
    );
    assert!(
        asked(&state).is_some_and(|(_, text)| text.contains("other package")),
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
    let state = m.state();
    assert!(
        matches!(
            slim_state(&state),
            (true, SlimWorker::Stopping { problem: Some(_) })
        ),
        "{state:?}"
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

    let mut asking = full();
    script(
        &mut asking,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_full(LightingTarget::Off), video(Video::Unknown)],
            ),
            (1, Input::SpawnFailed("no such file".into()), vec![]),
            (2, Input::Shutdown, vec![]),
        ],
    );
    assert!(asking.finished());
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
        m.handle(Input::Log(format!("line {line}")), t0);
    }
    let log = m.state().log;
    assert_eq!(log.len(), LOG_LINES);
    assert_eq!(log[0], "line 5");
    assert!(!m.handle(Pw(Unknown), t0).contains(&Effect::Publish));
    assert!(m.handle(Pw(Asleep), t0).contains(&Effect::Publish));
    assert!(
        m.handle(Input::Log("x".into()), t0)
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
            applied: 0,
            target: LightingTarget::ScreenSync,
            lights: None,
            portal: Some("http://10.0.0.2:8787/".into()),
            failure: None,
        }),
        t0,
    );
    let state = m.state();
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
    // Restarted: nothing of the old worker's report is left.
    m.handle(Exited(Exit::Code(1)), t0);
    let state = m.state();
    assert_eq!(
        full_state(&state),
        (
            LightingTarget::ScreenSync,
            &FullWorker::Running {
                capturing: false,
                fps: None,
                lights: None,
                portal: None,
                problem: None,
            }
        )
    );
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
                encoding: StreamEncoding::Raw,
                fps: 25,
            },
        }),
        t0,
    );
    let json = serde_json::to_value(m.state()).unwrap();
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
    assert!(full_capturing(&m.state()));
    script(
        &mut m,
        t0,
        vec![
            (
                2,
                Target(LightingTarget::Off),
                vec![send_target(LightingTarget::Off, 1)],
            ),
            (3, status_after(LightingTarget::Off, 1), vec![]),
        ],
    );
    let state = m.state();
    assert!(!full_capturing(&state));
    assert_eq!(full_state(&state).0, LightingTarget::Off);

    // A worker being stopped, or a late line from it, never shows capturing.
    script(
        &mut m,
        t0,
        vec![(
            4,
            Target(LightingTarget::ScreenSync),
            vec![send_target(LightingTarget::ScreenSync, 2)],
        )],
    );
    script(&mut m, t0, vec![(5, capturing.clone(), vec![])]);
    assert!(full_capturing(&m.state()));
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
        full_state(&m.state()).1,
        &FullWorker::Stopping {
            portal: Some("http://10.0.0.2:8787/".into()),
            problem: None,
        }
    );
}

/// What the person is asked is published in the phase that asks it, with
/// what is asked; a slim worker's waits and failures are its link's.
#[test]
fn what_is_asked_is_published_where_it_is_asked() {
    let mut m = slim(Preferences {
        autostart: true,
        ..slim_prefs()
    });
    let t0 = Instant::now();
    let waiting = SlimLink::Waiting {
        message: "waiting for the receiver: refused".into(),
    };
    script(
        &mut m,
        t0,
        vec![
            (
                0,
                Pw(Active),
                vec![spawn_slim(&receiver()), video(Video::Unknown)],
            ),
            (1, slim_link(waiting.clone()), vec![]),
        ],
    );
    let state = m.state();
    assert_eq!(asked(&state), None);
    assert_eq!(
        slim_state(&state).1,
        &SlimWorker::Running {
            capturing: false,
            link: waiting,
        }
    );
    script(
        &mut m,
        t0,
        vec![
            (
                3,
                Exited(Exit::Code(1)),
                vec![spawn_slim(&receiver()), video(Video::Unknown)],
            ),
            (4, Input::SpawnFailed("no such file".into()), vec![]),
        ],
    );
    assert_eq!(
        serde_json::to_value(m.state()).unwrap()["worker"],
        json!({
            "phase": "asking",
            "problem": {
                "asked": { "step": "ask", "fix": "try_again" },
                "message": "cannot start the worker: no such file",
            },
        })
    );
    // A running full worker keeps running with what it asks.
    for (escalation, json) in [
        (
            Escalation::Ask(Fix::ChangeSettings),
            json!({ "step": "ask", "fix": "change_settings" }),
        ),
        (Escalation::Unanswered, json!({ "step": "unanswered" })),
    ] {
        let mut full = full();
        full.handle(Pw(Active), t0);
        full.handle(
            full_status(FullStatus {
                failure: Some(failure(escalation, "x")),
                ..full_report(LightingTarget::Off)
            }),
            t0,
        );
        let worker = &serde_json::to_value(full.state()).unwrap()["worker"];
        assert_eq!(worker["phase"], "running");
        assert_eq!(worker["problem"], json!({ "asked": json, "message": "x" }));
    }
}

/// A receiver that paused the stream (its lighting is off) is not a
/// failure: the state says paused, and only a running worker can be
/// paused.
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
            (1_002, slim_link(SlimLink::Paused {}), vec![]),
        ],
    );
    let state = m.state();
    assert_eq!(
        slim_state(&state).1,
        &SlimWorker::Running {
            capturing: false,
            link: SlimLink::Paused {},
        }
    );
    assert_eq!(state.package.problem(), None);
    assert_eq!(
        serde_json::to_value(m.state()).unwrap()["worker"]["link"],
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
        slim_state(&m.state()).1,
        &SlimWorker::Stopping { problem: None },
        "a worker being stopped is not paused"
    );
}

/// Video the TV refuses to share is not a failure either.
#[test]
fn a_refused_video_is_published_without_a_failure() {
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
            (1_002, slim_link(SlimLink::Refused {}), vec![]),
        ],
    );
    let state = m.state();
    assert_eq!(state.package.problem(), None);
    assert_eq!(
        serde_json::to_value(&state).unwrap()["worker"]["link"],
        json!({ "state": "refused" })
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
                    prysm_address: Some(raw.clone()),
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

#[test]
fn a_requested_stop_clears_the_failure_in_every_phase() {
    let t0 = Instant::now();
    let asking = || {
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
                (1, failed_with(Escalation::FAIL_FAST, "no capture"), vec![]),
                (1, Exited(Exit::Code(1)), vec![]),
            ],
        );
        assert_eq!(asked(&m.state()), Some((TRY_AGAIN, "no capture")));
        m
    };
    for input in [Enabled(false), Pw(Asleep)] {
        let mut m = asking();
        script(&mut m, t0, vec![(2, input.clone(), vec![])]);
        let state = m.state();
        assert_eq!(slim_state(&state).1, &SlimWorker::Stopped, "{input:?}");
        assert_eq!(state.package.problem(), None, "{input:?}");
    }
    // Capture on, or a new address: the new worker starts without the old
    // one's failure.
    let mut m = asking();
    script(
        &mut m,
        t0,
        vec![(
            2,
            Enabled(true),
            vec![spawn_slim(&receiver()), video(Video::Unknown)],
        )],
    );
    assert_eq!(m.state().package.problem(), None);
    let mut m = asking();
    let other = other_receiver();
    script(
        &mut m,
        t0,
        vec![(
            2,
            Input::Preferences(Preferences {
                autostart: true,
                prysm_address: Some(other.clone()),
            }),
            vec![spawn_slim(&other), video(Video::Unknown)],
        )],
    );
    assert_eq!(m.state().package.problem(), None);
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
    let state = m.state();
    assert_eq!(slim_state(&state).1, &SlimWorker::Stopped);
    assert_eq!(state.package.problem(), None);
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
                slim_link(SlimLink::Waiting {
                    message: "waiting for the receiver: refused".into(),
                }),
                vec![],
            ),
            (
                2,
                Input::WorkerBroken("malformed".into()),
                vec![Effect::Terminate],
            ),
        ],
    );
    let state = m.state();
    assert_eq!(
        asked(&state),
        Some((TRY_AGAIN, "worker link broke: malformed"))
    );
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

const UNREADABLE: &str = "invalid tv.json";

/// Unreadable preferences stay reported through standby, a healthy worker
/// and its failures, beside what the person is asked, and end only when
/// preferences are saved (published).
#[test]
fn unreadable_preferences_are_reported_until_saved() {
    let t0 = Instant::now();
    let mut m = full().with_unreadable_preferences(UNREADABLE);
    let unreadable = |m: &Machine| m.state().unreadable.as_deref() == Some(UNREADABLE);
    assert!(unreadable(&m));
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
                Input::WorkerBroken("bad line".into()),
                vec![Effect::Terminate],
            ),
            (2, Exited(Exit::Code(1)), vec![]),
        ],
    );
    // Standby dismisses what the person is asked, not this.
    assert_eq!(
        asked(&m.state()),
        Some((TRY_AGAIN, "worker link broke: bad line"))
    );
    assert!(unreadable(&m));
    script(&mut m, t0, vec![(2, Pw(Asleep), vec![])]);
    assert!(unreadable(&m));
    let effects = m.handle(Input::Preferences(Preferences::default()), t0);
    assert_eq!(effects, [Effect::Publish]);
    assert_eq!(m.state().unreadable, None);
}

/// The full package's lighting inputs from each target, with the worker in
/// each phase: a target is kept, sent only to a running worker, and answers
/// what the person was asked (a new worker with it); the slim package's
/// `Enabled` changes nothing.
#[test]
fn every_full_target_takes_every_lighting_input() {
    use LightingTarget::{Effects, Off, ScreenSync};
    #[derive(Debug, Clone, Copy)]
    enum Where {
        Idle,
        Running,
        Stopping,
        Asking,
    }
    // Each input, and the target it chooses.
    let inputs = [
        (Target(Off), Some(Off)),
        (Target(ScreenSync), Some(ScreenSync)),
        (Target(Effects), Some(Effects)),
        (Enabled(false), None),
        (Enabled(true), None),
    ];
    let t0 = Instant::now();
    for from in LightingTarget::ALL {
        for phase in [Where::Idle, Where::Running, Where::Stopping, Where::Asking] {
            for (input, chosen) in inputs.clone() {
                let mut m = full();
                let mut steps = vec![(0, Target(from), vec![])];
                let start = vec![spawn_full(from), video(Video::Unknown)];
                match phase {
                    Where::Idle => {}
                    Where::Running => steps.push((0, Pw(Active), start)),
                    Where::Stopping => steps.extend([
                        (0, Pw(Active), start),
                        (
                            0,
                            Pw(Asleep),
                            vec![video(Video::Unavailable), Effect::Terminate],
                        ),
                    ]),
                    Where::Asking => steps.extend([
                        (0, Pw(Active), start),
                        (0, Input::SpawnFailed("no such file".into()), vec![]),
                    ]),
                }
                script(&mut m, t0, steps);
                let (effects, intent) = match (chosen, phase) {
                    (Some(target), Where::Running) => (vec![send_target(target, 1)], target),
                    (Some(target), Where::Asking) => {
                        (vec![spawn_full(target), video(Video::Unknown)], target)
                    }
                    (Some(target), Where::Idle | Where::Stopping) => (vec![], target),
                    (None, Where::Idle | Where::Running | Where::Stopping | Where::Asking) => {
                        (vec![], from)
                    }
                };
                let shown = format!("{from:?}, {phase:?}, {input:?}");
                let done: Vec<_> = m
                    .handle(input, t0 + Duration::from_millis(1))
                    .into_iter()
                    .filter(|effect| *effect != Effect::Publish)
                    .collect();
                assert_eq!(done, effects, "{shown}");
                assert_eq!(m.intent(), intent, "{shown}");
                assert_eq!(full_state(&m.state()).0, intent, "{shown}");
            }
        }
    }
}

/// The only timers: a running worker's silence, a stopping one's kill and
/// letting go. Nothing is due without a worker, asked or not.
#[test]
fn every_phase_says_when_it_is_next_due() {
    let t0 = Instant::now();
    let at = |ms| t0 + Duration::from_millis(ms);
    let mut m = full();
    assert_eq!(m.next_due(), None, "idle: only an input changes anything");
    m.handle(Pw(Active), t0);
    assert_eq!(m.next_due(), Some(t0 + SILENCE), "running, not heard");
    m.handle(status(LightingTarget::Off), at(50_000));
    assert_eq!(m.next_due(), Some(at(50_000) + SILENCE), "heard");
    m.handle(Pw(Asleep), at(61_000));
    assert_eq!(m.next_due(), Some(at(61_000) + KILL_AFTER), "the kill");
    m.handle(Tick, at(61_000) + KILL_AFTER);
    assert_eq!(
        m.next_due(),
        Some(at(61_000) + KILL_AFTER * 2),
        "killed: letting go"
    );

    let mut m = full();
    m.handle(Pw(Active), t0);
    m.handle(Input::SpawnFailed("no such file".into()), at(1));
    assert_eq!(m.next_due(), None, "asking");
}
