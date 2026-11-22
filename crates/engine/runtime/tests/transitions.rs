//! Every cell of the engine's transition function: each lighting state it can
//! be in between commands, with the target that led there, against each
//! command that drives it (a target, or a save changing processing, the
//! capture or the output). Each row names the adapter calls the command
//! makes and the state it leaves.
//!
//! A save changing the output always changes the adapters too, so the
//! transition's `output_changed` without `restart` cannot be reached and has
//! no row.
use prysm_ports::{
    AdapterSettings, Error, LightingTarget, Processing, Registry, SessionPhase, SessionState,
    Settings,
};
use prysm_runtime::fakes::{
    CallLog, FakeOutputSettings, FakeOutputs, FakeSourceSettings, FakeSources, MemoryStore,
};
use prysm_runtime::{Engine, EngineHandle};
use std::time::Duration;

const DEADLINE: Duration = Duration::from_secs(2);

/// Where a row starts, and how the engine got there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    /// Target `Off`, nothing touched.
    Off,
    /// Target `Processed`, the session holding the output.
    Running,
    /// Target `DeviceNative`, the output in its own effects.
    Native,
    /// Target `Processed`, asked from `Native` with a source that cannot
    /// open: the effects still show.
    ProcessedFromNative,
    /// Target `Processed` with a source that cannot open.
    ProcessedFailed,
    /// Target `DeviceNative` with an output that cannot idle.
    NativeFailed,
    /// A session whose source release is unconfirmed.
    Unreleased,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Target(LightingTarget),
    /// A save of other processing: same adapters.
    Processing,
    /// A save of another capture, source `s2`.
    Capture,
    /// A save of another output, `o2`.
    Output,
}

/// The command's result: done, failed (reported in the snapshot), or
/// refused because the engine is faulted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Done,
    Failed,
    Refused,
}

use Command::{Capture, Output, Processing as Process, Target};
use LightingTarget::{DeviceNative, Off, Processed};
use Outcome::{Done, Failed, Refused};

const RUNNING: SessionState = SessionState::Running;
const STOPPED: SessionState = SessionState::Stopped;
const NATIVE: SessionState = SessionState::Native;
const UNRELEASED: SessionState = SessionState::Faulted(SessionPhase::SourceUnreleased);

type Row = (
    Start,
    Command,
    &'static [&'static str],
    SessionState,
    Outcome,
);

const CASES: &[Row] = &[
    (Start::Off, Target(Off), &[], STOPPED, Done),
    (
        Start::Off,
        Target(Processed),
        &["source.open:s", "output.open:o"],
        RUNNING,
        Done,
    ),
    (
        Start::Off,
        Target(DeviceNative),
        &["output.open:o", "output.idle:o:Native"],
        NATIVE,
        Done,
    ),
    (Start::Off, Process, &[], STOPPED, Done),
    (Start::Off, Capture, &[], STOPPED, Done),
    (Start::Off, Output, &[], STOPPED, Done),
    //
    (
        Start::Running,
        Target(Off),
        &["source.close:s", "output.idle:o:Off"],
        STOPPED,
        Done,
    ),
    (Start::Running, Target(Processed), &[], RUNNING, Done),
    (
        Start::Running,
        Target(DeviceNative),
        &["source.close:s", "output.idle:o:Native"],
        NATIVE,
        Done,
    ),
    (Start::Running, Process, &[], RUNNING, Done),
    (
        Start::Running,
        Capture,
        &[
            "source.close:s",
            "output.idle:o:Off",
            "source.open:s2",
            "output.open:o",
        ],
        RUNNING,
        Done,
    ),
    (
        Start::Running,
        Output,
        &[
            "source.close:s",
            "output.idle:o:Off",
            "source.open:s",
            "output.open:o2",
        ],
        RUNNING,
        Done,
    ),
    //
    (
        Start::Native,
        Target(Off),
        &["output.open:o", "output.idle:o:Off"],
        STOPPED,
        Done,
    ),
    (
        Start::Native,
        Target(Processed),
        &["source.open:s", "output.open:o"],
        RUNNING,
        Done,
    ),
    (Start::Native, Target(DeviceNative), &[], NATIVE, Done),
    (Start::Native, Process, &[], NATIVE, Done),
    (Start::Native, Capture, &[], NATIVE, Done),
    (
        Start::Native,
        Output,
        &["output.open:o2", "output.idle:o2:Native"],
        NATIVE,
        Done,
    ),
    //
    (
        Start::ProcessedFromNative,
        Target(Off),
        &["output.open:o", "output.idle:o:Off"],
        STOPPED,
        Done,
    ),
    (
        Start::ProcessedFromNative,
        Target(Processed),
        &["source.open_failed:s"],
        NATIVE,
        Failed,
    ),
    (
        Start::ProcessedFromNative,
        Target(DeviceNative),
        &[],
        NATIVE,
        Done,
    ),
    (
        Start::ProcessedFromNative,
        Process,
        &["source.open_failed:s"],
        NATIVE,
        Failed,
    ),
    (
        Start::ProcessedFromNative,
        Capture,
        &["source.open:s2", "output.open:o"],
        RUNNING,
        Done,
    ),
    (
        Start::ProcessedFromNative,
        Output,
        &["source.open_failed:s"],
        NATIVE,
        Failed,
    ),
    //
    (Start::ProcessedFailed, Target(Off), &[], STOPPED, Done),
    (
        Start::ProcessedFailed,
        Target(Processed),
        &["source.open_failed:s"],
        STOPPED,
        Failed,
    ),
    (
        Start::ProcessedFailed,
        Target(DeviceNative),
        &["output.open:o", "output.idle:o:Native"],
        NATIVE,
        Done,
    ),
    (
        Start::ProcessedFailed,
        Process,
        &["source.open_failed:s"],
        STOPPED,
        Failed,
    ),
    (
        Start::ProcessedFailed,
        Capture,
        &["source.open:s2", "output.open:o"],
        RUNNING,
        Done,
    ),
    (
        Start::ProcessedFailed,
        Output,
        &["source.open_failed:s"],
        STOPPED,
        Failed,
    ),
    //
    (Start::NativeFailed, Target(Off), &[], STOPPED, Done),
    (
        Start::NativeFailed,
        Target(Processed),
        &["source.open:s", "output.open:o"],
        RUNNING,
        Done,
    ),
    (
        Start::NativeFailed,
        Target(DeviceNative),
        &["output.open:o", "output.idle:o:Native"],
        STOPPED,
        Failed,
    ),
    (
        Start::NativeFailed,
        Process,
        &["output.open:o", "output.idle:o:Native"],
        STOPPED,
        Failed,
    ),
    (
        Start::NativeFailed,
        Capture,
        &["output.open:o", "output.idle:o:Native"],
        STOPPED,
        Failed,
    ),
    (
        Start::NativeFailed,
        Output,
        &["output.open:o2", "output.idle:o2:Native"],
        NATIVE,
        Done,
    ),
    //
    (Start::Unreleased, Target(Off), &[], UNRELEASED, Refused),
    (
        Start::Unreleased,
        Target(Processed),
        &[],
        UNRELEASED,
        Refused,
    ),
    (
        Start::Unreleased,
        Target(DeviceNative),
        &[],
        UNRELEASED,
        Refused,
    ),
    (Start::Unreleased, Process, &[], UNRELEASED, Refused),
    (Start::Unreleased, Capture, &[], UNRELEASED, Refused),
    (Start::Unreleased, Output, &[], UNRELEASED, Refused),
];

fn base(from: Start) -> Settings {
    let mut source = FakeSourceSettings {
        name: "s".into(),
        ..Default::default()
    };
    let mut output = FakeOutputSettings {
        name: "o".into(),
        ..Default::default()
    };
    match from {
        Start::Off | Start::Running | Start::Native => {}
        Start::ProcessedFromNative | Start::ProcessedFailed => source.fail_open = true,
        Start::NativeFailed => output.fail_idle = true,
        Start::Unreleased => source.fail_close = true,
    }
    Settings {
        capture: Some(AdapterSettings::new("fake", &source)),
        output: Some(AdapterSettings::new("fake", &output)),
        processing: Processing::default(),
    }
}

/// The saved settings `command` changes `settings` to, if it is a save.
fn saved(command: Command, mut settings: Settings) -> Option<Settings> {
    match command {
        Command::Target(_) => return None,
        Command::Processing => settings.processing.brightness_percent = 40,
        Command::Capture => {
            let source = FakeSourceSettings {
                name: "s2".into(),
                ..Default::default()
            };
            settings.capture = Some(AdapterSettings::new("fake", &source));
        }
        Command::Output => {
            let output = FakeOutputSettings {
                name: "o2".into(),
                ..Default::default()
            };
            settings.output = Some(AdapterSettings::new("fake", &output));
        }
    }
    Some(settings)
}

/// Drive a new engine into `from`.
async fn reach(from: Start, log: &CallLog) -> (EngineHandle, Engine) {
    let mut registry = Registry::default();
    registry.add_source(FakeSources(log.clone()));
    registry.add_output(FakeOutputs(log.clone()));
    let store = MemoryStore::with(base(from));
    let (handle, engine) = Engine::start(registry, Box::new(store), DEADLINE)
        .await
        .unwrap();
    let set = |target| {
        let handle = handle.clone();
        async move { handle.set_target(target).await }
    };
    match from {
        Start::Off => {}
        Start::Running => {
            set(Processed).await.unwrap();
            assert!(log.wait_for("output.write:o", 1).await);
        }
        Start::Native => set(DeviceNative).await.unwrap(),
        Start::ProcessedFromNative => {
            set(DeviceNative).await.unwrap();
            set(Processed).await.unwrap_err();
        }
        Start::ProcessedFailed => drop(set(Processed).await.unwrap_err()),
        Start::NativeFailed => drop(set(DeviceNative).await.unwrap_err()),
        Start::Unreleased => {
            set(Processed).await.unwrap();
            assert!(log.wait_for("output.write:o", 1).await);
            set(Off).await.unwrap_err();
        }
    }
    (handle, engine)
}

/// The calls since `mark`, without the writes a running session repeats.
fn calls_since(log: &CallLog, mark: usize) -> Vec<String> {
    log.events()[mark..]
        .iter()
        .filter(|call| !call.starts_with("output.write:"))
        .cloned()
        .collect()
}

/// A session started by the command opens its output at its first frame,
/// after the command returned: wait up to five seconds for `count` calls.
async fn settled(log: &CallLog, mark: usize, count: usize) -> Vec<String> {
    for _ in 0..1000 {
        let calls = calls_since(log, mark);
        if calls.len() >= count {
            return calls;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    calls_since(log, mark)
}

/// What the reply and the snapshot's failure say together. A save replies
/// with its revision and reports a failed apply in the snapshot; a target
/// replies with the failure it records.
fn outcome(reply: &prysm_ports::Result<()>, failure: Option<&Error>, save: bool) -> Outcome {
    match (reply, failure) {
        (Err(Error::Faulted(_)), _) => Refused,
        (Err(_), Some(_)) => Failed,
        (Ok(()), Some(_)) if save => Failed,
        (Ok(()), None) => Done,
        other => panic!("a reply and a recorded failure that disagree: {other:?}"),
    }
}

/// The last `<prefix><name>` call in `calls`, as `<name>`.
fn last_named<'a>(calls: &'a [String], prefix: &str) -> &'a str {
    calls
        .iter()
        .rev()
        .find_map(|call| call.strip_prefix(prefix))
        .expect("a call with that prefix")
}

async fn run(row: &Row) {
    let &(from, command, calls, state, expected) = row;
    let log = CallLog::new();
    let (handle, engine) = reach(from, &log).await;
    let mark = log.events().len();
    let save = saved(command, base(from));
    let is_save = save.is_some();
    let reply = match save {
        None => {
            let Command::Target(target) = command else {
                unreachable!("only a target is not a save")
            };
            handle.set_target(target).await
        }
        Some(settings) => {
            let revision = handle.snapshot().borrow().revision;
            handle.replace_settings(revision, settings).await.map(drop)
        }
    };
    let made = settled(&log, mark, calls.len()).await;
    let snapshot = handle.snapshot().borrow().clone();
    let got = outcome(&reply, snapshot.failure.as_ref(), is_save);
    assert_eq!(made, calls, "{row:?}");
    assert_eq!(snapshot.session, state, "{row:?}");
    assert_eq!(got, expected, "{row:?}: {reply:?}, {:?}", snapshot.failure);

    // Shutdown runs after everything the command started: the log then holds
    // the row's calls and the shutdown's own, and nothing that came late.
    let _ = engine.shutdown().await;
    let mut after: Vec<String> = calls.iter().map(|&call| call.to_owned()).collect();
    if state == RUNNING {
        let all = calls_since(&log, 0);
        after.push(format!("source.close:{}", last_named(&all, "source.open:")));
        after.push(format!(
            "output.idle:{}:Off",
            last_named(&all, "output.open:")
        ));
    }
    assert_eq!(calls_since(&log, mark), after, "{row:?} after shutdown");
}

#[tokio::test]
async fn every_state_meets_every_command() {
    tokio::time::timeout(Duration::from_secs(60), async {
        for row in CASES {
            run(row).await;
        }
    })
    .await
    .expect("the table timed out");
}

/// Every row's start and command is in the table, once.
#[test]
fn the_table_covers_every_cell() {
    let starts = [
        Start::Off,
        Start::Running,
        Start::Native,
        Start::ProcessedFromNative,
        Start::ProcessedFailed,
        Start::NativeFailed,
        Start::Unreleased,
    ];
    let commands = [
        Target(Off),
        Target(Processed),
        Target(DeviceNative),
        Process,
        Capture,
        Output,
    ];
    for from in starts {
        for command in commands {
            let found = CASES
                .iter()
                .filter(|(f, c, ..)| *f == from && *c == command)
                .count();
            assert_eq!(found, 1, "{from:?} x {command:?}");
        }
    }
}

/// Faulted, the engine refuses every command but shutdown, and changes nothing.
#[tokio::test]
async fn a_faulted_engine_refuses_every_command() {
    let log = CallLog::new();
    let (handle, engine) = reach(Start::Unreleased, &log).await;
    let calls = log.events();
    let revision = handle.snapshot().borrow().revision;
    let refused = |result: prysm_ports::Result<()>| {
        assert!(matches!(result, Err(Error::Faulted(_))), "{result:?}");
    };
    refused(handle.set_calibration(None).await);
    refused(handle.try_processing(Processing::default()).await);
    refused(handle.set_target(Off).await);
    refused(
        handle
            .replace_settings(revision, base(Start::Off))
            .await
            .map(drop),
    );
    assert_eq!(log.events(), calls);
    assert_eq!(handle.snapshot().borrow().revision, revision);
    assert!(engine.shutdown().await.is_err());
}

/// How the session in a race row ends on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnEnd {
    /// The source fails; its release is confirmed or not.
    Failed { released: bool },
    /// The source ends; its release is confirmed or not.
    Ended { released: bool },
}

/// What meets the session's own end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Meets {
    Target(LightingTarget),
    /// A save of another capture: a restart.
    Capture,
    Shutdown,
}

/// Whether the engine handles the session's end before the command, or the
/// command arrives while the thread is still idling the output after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Order {
    EndFirst,
    CommandFirst,
}

/// Everything a race row can observe.
#[derive(Debug, PartialEq)]
struct Seen {
    reply: Result<(), String>,
    session: SessionState,
    target: LightingTarget,
    failure: Option<String>,
    saved: Option<Settings>,
    shutdown: Result<(), String>,
    calls: Vec<String>,
}

fn racing(end: OwnEnd) -> Settings {
    let (failed, released) = match end {
        OwnEnd::Failed { released } => (true, released),
        OwnEnd::Ended { released } => (false, released),
    };
    let source = FakeSourceSettings {
        name: "s".into(),
        fail_after: failed.then_some(2),
        end_after: (!failed).then_some(2),
        fail_close: !released,
        ..Default::default()
    };
    // Idling takes long enough for a command to arrive meanwhile.
    let output = FakeOutputSettings {
        name: "o".into(),
        idle_delay_ms: 200,
        ..Default::default()
    };
    Settings {
        capture: Some(AdapterSettings::new("fake", &source)),
        output: Some(AdapterSettings::new("fake", &output)),
        processing: Processing::default(),
    }
}

fn text(result: prysm_ports::Result<()>) -> Result<(), String> {
    result.map_err(|error| error.to_string())
}

async fn race(end: OwnEnd, meets: Meets, order: Order) -> Seen {
    let log = CallLog::new();
    let mut registry = Registry::default();
    registry.add_source(FakeSources(log.clone()));
    registry.add_output(FakeOutputs(log.clone()));
    let store = MemoryStore::with(racing(end));
    let (handle, engine) = Engine::start(registry, Box::new(store.clone()), DEADLINE)
        .await
        .unwrap();
    handle.set_target(Processed).await.unwrap();
    assert!(log.wait_for("output.idle:o:Off", 1).await);
    if order == Order::EndFirst {
        handle
            .snapshot()
            .wait_for(|s| s.session != SessionState::Running)
            .await
            .unwrap();
    }
    let opened = log.count("output.open:o");
    let reply = match meets {
        Meets::Target(target) => handle.set_target(target).await,
        Meets::Capture => {
            let mut settings = racing(end);
            let source = FakeSourceSettings {
                name: "s2".into(),
                ..Default::default()
            };
            settings.capture = Some(AdapterSettings::new("fake", &source));
            let revision = handle.snapshot().borrow().revision;
            handle.replace_settings(revision, settings).await.map(drop)
        }
        Meets::Shutdown => Ok(()),
    };
    let (reply, snapshot, shutdown) = if meets == Meets::Shutdown {
        let shutdown = text(engine.shutdown().await);
        (
            shutdown.clone(),
            handle.snapshot().borrow().clone(),
            shutdown,
        )
    } else {
        let snapshot = handle.snapshot().borrow().clone();
        // A session the command started takes the output at its first frame,
        // however late that comes: shut down only after it did.
        if snapshot.session == SessionState::Running {
            assert!(log.wait_for("output.open:o", opened + 1).await);
        }
        (text(reply), snapshot, text(engine.shutdown().await))
    };
    Seen {
        reply,
        session: snapshot.session,
        target: snapshot.target,
        failure: snapshot.failure.map(|failure| failure.to_string()),
        saved: store.saved(),
        shutdown,
        calls: calls_since(&log, 0),
    }
}

/// A command that meets a session which ended on its own has one outcome,
/// whichever the engine saw first: the same reply, state, failure, saved
/// settings, shutdown and adapter calls. An unconfirmed release faults the
/// engine either way.
#[tokio::test]
async fn a_command_meeting_an_own_end_has_one_outcome() {
    let ends = [
        OwnEnd::Failed { released: true },
        OwnEnd::Failed { released: false },
        OwnEnd::Ended { released: true },
        OwnEnd::Ended { released: false },
    ];
    let meets = [
        Meets::Target(Off),
        Meets::Target(DeviceNative),
        Meets::Capture,
        Meets::Shutdown,
    ];
    let cases: Vec<(OwnEnd, Meets)> = ends
        .iter()
        .flat_map(|&end| meets.iter().map(move |&meets| (end, meets)))
        .collect();
    let seen = tokio::time::timeout(
        Duration::from_secs(60),
        futures::future::join_all(cases.iter().map(|&(end, meets)| async move {
            let first = race(end, meets, Order::EndFirst).await;
            let later = race(end, meets, Order::CommandFirst).await;
            (first, later)
        })),
    )
    .await
    .expect("the races timed out");
    for (&(end, meets), (first, later)) in cases.iter().zip(seen) {
        assert_eq!(first, later, "{end:?} x {meets:?}");
        if matches!(
            end,
            OwnEnd::Failed { released: false } | OwnEnd::Ended { released: false }
        ) {
            assert_eq!(first.session, UNRELEASED, "{end:?} x {meets:?}");
            assert!(first.shutdown.is_err(), "{end:?} x {meets:?}: {first:?}");
        }
    }
}
