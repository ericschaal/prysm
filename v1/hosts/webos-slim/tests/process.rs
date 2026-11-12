//! The built `prysm-webos-slim` binary on the development host, where the LG
//! backend refuses to start: argv and failure exit codes with a final status.
use std::net::TcpListener;
use std::process::Command;

const BINARY: &str = env!("CARGO_BIN_EXE_prysm-webos-slim");

fn args(receiver: &str) -> Vec<String> {
    [
        "--receiver",
        receiver,
        "--width",
        "320",
        "--height",
        "180",
        "--fps",
        "30",
        "--include-ui",
        "true",
    ]
    .map(str::to_owned)
    .to_vec()
}

#[test]
fn bad_arguments_exit_with_usage() {
    let mut odd = args("127.0.0.1:7380");
    odd[3] = "321".into();
    for argv in [vec![], odd, vec!["slim".to_owned()]] {
        let output = Command::new(BINARY).args(&argv).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{argv:?}");
        assert!(output.stdout.is_empty());
    }
}

fn final_failure(output: &std::process::Output) -> String {
    let text = String::from_utf8_lossy(&output.stdout);
    let last = text.lines().last().expect("a final status");
    let event: serde_json::Value = serde_json::from_str(last).unwrap();
    assert_eq!(event["type"], "status", "{last}");
    event["failure"].as_str().unwrap().to_owned()
}

#[test]
fn an_unreachable_receiver_fails_before_capture() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let output = Command::new(BINARY)
        .args(args(&format!("127.0.0.1:{port}")))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(final_failure(&output).contains("cannot connect to receiver"));
}

#[test]
fn capture_off_the_tv_fails_with_its_cause() {
    let receiver = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = receiver.local_addr().unwrap().to_string();
    let output = Command::new(BINARY).args(args(&address)).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(final_failure(&output).contains("webOS"));
}
