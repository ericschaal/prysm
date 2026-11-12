//! The built `prysm-webos` binary on the development host: argv, events on
//! stdout, exit codes. The LG backend is never opened (lighting stays off).
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BINARY: &str = env!("CARGO_BIN_EXE_prysm-webos");

fn wait(child: &mut std::process::Child) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status.code().expect("exit code");
        }
        assert!(Instant::now() < deadline, "worker did not exit");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn bad_arguments_exit_with_usage() {
    for args in [&[][..], &["--settings", "x"][..], &["full"][..]] {
        let output = Command::new(BINARY).args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "no events before the link exists");
        assert!(String::from_utf8_lossy(&output.stderr).contains("usage"));
    }
}

fn spawn(settings: &std::path::Path) -> std::process::Child {
    Command::new(BINARY)
        .args(["--settings", settings.to_str().unwrap()])
        .args(["--listen", "127.0.0.1:0", "--lighting", "off"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

#[test]
fn reports_status_and_stops_on_stdin_eof() {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("settings.json");
    let mut child = spawn(&settings);
    let mut events = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    events.read_line(&mut line).unwrap();
    let event: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(event["type"], "status", "{line}");
    assert_eq!(event["target"], "off");
    assert_eq!(event["capturing"], false);
    let portal = event["portal"].as_str().unwrap();
    assert!(portal.starts_with("http://127.0.0.1:"), "{portal}");
    assert!(
        std::fs::read_to_string(&settings)
            .unwrap()
            .contains("webos")
    );

    drop(child.stdin.take());
    assert_eq!(wait(&mut child), 0);
}

#[test]
fn a_garbage_command_fails_the_worker() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = spawn(&dir.path().join("settings.json"));
    let mut events = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    events.read_line(&mut line).unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"{\"type\":\"reboot\"}\n").unwrap();
    assert_eq!(wait(&mut child), 1);
}
