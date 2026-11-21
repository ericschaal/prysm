//! The real launcher against `sh` scripts standing in for the worker.
use super::*;
use std::time::{Duration, Instant};

fn sh(script: &str) -> ProcessWorker {
    ProcessLauncher
        .spawn(Path::new("/bin/sh"), &["-c".into(), script.into()])
        .unwrap()
}

/// Collect outputs until `done` holds for them, within five seconds.
fn collect(worker: &mut ProcessWorker, done: impl Fn(&[Output]) -> bool) -> Vec<Output> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = Vec::new();
    while !done(&seen) {
        assert!(Instant::now() < deadline, "only saw {seen:?}");
        seen.extend(worker.drain());
        std::thread::sleep(Duration::from_millis(10));
    }
    seen
}

fn exit_within(worker: &mut ProcessWorker, limit: Duration) -> Option<Exit> {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if let Some(exit) = worker.try_exit().unwrap() {
            return Some(exit);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

#[test]
fn events_logs_commands_and_exit_code() {
    let mut worker = sh(
        r#"echo '{"type":"status","flavor":"full","capturing":true,"fps":null,"target":"off","portal":null,"failure":null}'
echo started >&2
read command
echo "got $command" >&2
exit 3"#,
    );
    worker.send(&Command::ResetVideo {}).unwrap();
    let seen = collect(&mut worker, |seen| seen.len() >= 3);
    // stdout and stderr are read on separate threads: no order between them.
    assert!(seen.iter().any(|output| matches!(
        output,
        Output::Event(Event::Status(prysm_webos_link::WorkerStatus::Full(status)))
            if status.capturing
    )));
    assert!(seen.contains(&Output::Log("started".into())));
    assert!(seen.contains(&Output::Log(r#"got {"type":"reset_video"}"#.into())));
    assert_eq!(
        exit_within(&mut worker, Duration::from_secs(5)),
        Some(Exit::Code(3))
    );
    assert!(
        worker.send(&Command::ResetVideo {}).is_err(),
        "stdin closes on reap"
    );
    assert_eq!(worker.try_exit().unwrap(), None, "reaped once");
}

/// An event still in the pipe when the worker is reaped (here written by a
/// process it left behind) is drained before the exit is handed over.
#[test]
fn the_last_events_come_before_the_exit() {
    let mut worker = sh(r#"(sleep 0.2; echo '{"type":"failed","cause":"late"}') &
exit 1"#);
    assert_eq!(
        exit_within(&mut worker, Duration::from_secs(5)),
        Some(Exit::Code(1))
    );
    let seen = worker.drain();
    assert!(
        seen.contains(&Output::Event(Event::Failed {
            cause: "late".into()
        })),
        "{seen:?}"
    );
}

/// A process the worker left behind that keeps its pipes open delays the
/// exit by at most [`LAST_WORDS`].
#[test]
fn a_left_behind_process_holding_the_pipes_delays_the_exit_boundedly() {
    let mut worker = sh("sleep 3 & exit 4");
    let started = Instant::now();
    assert_eq!(
        exit_within(&mut worker, Duration::from_secs(5)),
        Some(Exit::Code(4))
    );
    let waited = started.elapsed();
    assert!(waited >= LAST_WORDS, "{waited:?}");
    assert!(waited < LAST_WORDS + Duration::from_secs(1), "{waited:?}");
}

#[test]
fn a_worker_ignoring_term_is_killed() {
    let mut worker = sh("trap '' TERM; echo ready >&2; while :; do sleep 0.05; done");
    collect(&mut worker, |seen| !seen.is_empty());
    worker.terminate().unwrap();
    assert_eq!(exit_within(&mut worker, Duration::from_millis(300)), None);
    worker.kill().unwrap();
    assert_eq!(
        exit_within(&mut worker, Duration::from_secs(5)),
        Some(Exit::Signal(9))
    );
}

#[test]
fn term_stops_a_cooperative_worker() {
    let mut worker = sh("trap 'exit 0' TERM; echo ready >&2; while :; do sleep 0.05; done");
    collect(&mut worker, |seen| !seen.is_empty());
    worker.terminate().unwrap();
    assert_eq!(
        exit_within(&mut worker, Duration::from_secs(5)),
        Some(Exit::Code(0))
    );
}

#[test]
fn garbage_and_oversized_lines_break_the_stream() {
    let mut worker = sh("echo '{\"type\":\"hello\"}'; sleep 5");
    let seen = collect(&mut worker, |seen| !seen.is_empty());
    assert!(
        matches!(&seen[0], Output::Broken(why) if why.contains("malformed")),
        "{seen:?}"
    );
    worker.kill().unwrap();
    exit_within(&mut worker, Duration::from_secs(5)).unwrap();

    let mut worker = sh("head -c 5000 /dev/zero | tr '\\0' x; echo; sleep 5");
    let seen = collect(&mut worker, |seen| !seen.is_empty());
    assert!(
        matches!(&seen[0], Output::Broken(why) if why.contains("exceeds")),
        "{seen:?}"
    );
    worker.kill().unwrap();
    exit_within(&mut worker, Duration::from_secs(5)).unwrap();
}

#[test]
fn long_log_lines_are_cut() {
    let mut worker = sh("head -c 2000 /dev/zero | tr '\\0' y >&2; echo >&2; echo done >&2");
    let seen = collect(&mut worker, |seen| seen.len() >= 2);
    assert!(matches!(&seen[0], Output::Log(line) if line.len() == MAX_LOG_LINE));
    assert_eq!(seen[1], Output::Log("done".into()));
    exit_within(&mut worker, Duration::from_secs(5)).unwrap();
}

#[test]
fn a_missing_program_fails_to_spawn() {
    assert!(
        ProcessLauncher
            .spawn(Path::new("/nonexistent/prysm-webos"), &[])
            .is_err()
    );
}

#[test]
fn dropping_an_unreaped_worker_terminates_it() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("terminated");
    let mut worker = sh(&format!(
        "trap 'echo > {}; exit 0' TERM; echo ready >&2; while :; do sleep 0.05; done",
        marker.display()
    ));
    collect(&mut worker, |seen| !seen.is_empty());
    let pid =
        nix::unistd::Pid::from_raw(i32::try_from(worker.child.as_ref().unwrap().id()).unwrap());
    drop(worker);
    let deadline = Instant::now() + Duration::from_secs(5);
    while nix::sys::signal::kill(pid, None).is_ok() {
        assert!(Instant::now() < deadline, "worker survived its handle");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists(), "stopped with SIGTERM, not killed");
}
