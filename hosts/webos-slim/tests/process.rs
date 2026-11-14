//! The built `prysm-webos-slim` binary on the development host, where the LG
//! backend refuses to start: argv, the wait for Prysm and the failure exit
//! code with a final status.
use prysm_capture_remote::StreamRequest;
use prysm_capture_remote::testkit::Portal;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BINARY: &str = env!("CARGO_BIN_EXE_prysm-webos-slim");

#[test]
fn bad_arguments_exit_with_usage() {
    let old = ["--receiver", "127.0.0.1:7380"].map(str::to_owned);
    let extra = ["--prysm", "http://127.0.0.1:8787", "--width", "320"].map(str::to_owned);
    let bare = ["--prysm", "127.0.0.1:8787"].map(str::to_owned);
    for argv in [
        vec![],
        old.to_vec(),
        extra.to_vec(),
        bare.to_vec(),
        vec!["slim".to_owned()],
    ] {
        let output = Command::new(BINARY).args(&argv).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{argv:?}");
        assert!(output.stdout.is_empty());
    }
}

/// The worker for the Prysm at `address`, with stdin kept open as the
/// supervisor does.
fn spawn(address: &str) -> Child {
    Command::new(BINARY)
        .args(["--prysm", address])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

/// Status lines until `until` matches one; the process is killed after 20 s.
fn statuses(child: &mut Child, until: impl Fn(&serde_json::Value) -> bool) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    loop {
        assert!(Instant::now() < deadline, "no matching status");
        let line = lines.next().expect("a status line").unwrap();
        let event: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(event["type"], "status", "{line}");
        if until(&event) {
            return event;
        }
    }
}

fn wait(child: &mut Child) -> Option<i32> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status.code();
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            panic!("the worker did not exit");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The receiver listens only while its host is lighting, so an unreachable
/// one is a reported wait: nothing is captured, the worker keeps running
/// and stops cleanly when stdin closes.
#[test]
fn an_unreachable_receiver_is_a_reported_wait() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut child = spawn(&format!("http://127.0.0.1:{port}"));
    let status = statuses(&mut child, |event| !event["failure"].is_null());
    assert_eq!(status["failure"]["kind"], "receiver", "{status}");
    assert!(
        status["failure"]["message"]
            .as_str()
            .unwrap()
            .starts_with("waiting for the receiver"),
        "{status}"
    );
    drop(child.stdin.take());
    assert_eq!(wait(&mut child), Some(0));
}

/// Once a receiver sends its request, the capture opens; off the TV that
/// fails the worker with its cause.
#[test]
fn capture_off_the_tv_fails_with_its_cause() {
    let portal = Portal::start();
    let receiver = portal.source(StreamRequest::default());
    let mut child = spawn(&format!("http://{}", portal.addr));
    let stdin = child.stdin.take();
    let status = statuses(&mut child, |event| event["failure"]["kind"] == "other");
    assert!(
        status["failure"]["message"]
            .as_str()
            .unwrap()
            .contains("webOS"),
        "{status}"
    );
    assert_eq!(wait(&mut child), Some(1));
    drop(stdin);
    drop(receiver);
}

/// The on-TV encoder check runs the software encoder anywhere; off the TV the
/// hardware one fails with its cause.
#[test]
fn check_encoder_reports_the_encoder_it_used() {
    let output = Command::new(BINARY)
        .args([
            "check-encoder",
            "--encoding",
            "h264_software",
            "--frames",
            "6",
            "--fps",
            "60",
        ])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{text}");
    assert!(
        text.contains("check-encoder: ok encoder=software H.264"),
        "{text}"
    );

    let output = Command::new(BINARY)
        .args(["check-encoder", "--frames", "2"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        text.contains("FAILED") && text.contains("hardware"),
        "{text}"
    );
    let usage = Command::new(BINARY)
        .args(["check-encoder", "--width", "3"])
        .output()
        .unwrap();
    assert_eq!(usage.status.code(), Some(2));
}
