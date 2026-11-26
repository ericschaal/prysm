//! The built `prysm-webos` binary on the development host: argv, events on
//! stdout, exit codes. The LG backend is never opened (lighting stays off).
use prysm_testkit::wait_child;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::Duration;

const BINARY: &str = env!("CARGO_BIN_EXE_prysm-webos");

/// How long the worker may take to exit; it is killed then.
const EXIT: Duration = Duration::from_secs(10);

#[test]
fn bad_arguments_fail() {
    for args in [&[][..], &["--settings", "x"][..], &["full"][..]] {
        let output = Command::new(BINARY).args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty(), "no events before the link exists");
        assert!(String::from_utf8_lossy(&output.stderr).contains("Usage:"));
    }
}

/// Taken over before parsing, fd 1 is stderr by the time clap prints help.
#[test]
fn help_goes_to_stderr() {
    let output = Command::new(BINARY).arg("--help").output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Usage:"));
}

fn spawn(settings: &std::path::Path) -> std::process::Child {
    Command::new(BINARY)
        .args(["--settings", settings.to_str().unwrap()])
        .args(["--listen", "127.0.0.1:0", "--target", "off"])
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
    assert_eq!(event["flavor"], "full", "{line}");
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
    assert_eq!(
        wait_child(&mut child, EXIT)
            .expect("the worker exits")
            .code(),
        Some(0)
    );
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
    assert_eq!(
        wait_child(&mut child, EXIT)
            .expect("the worker exits")
            .code(),
        Some(1)
    );
}

/// The status line of `method` on `/api/v1/state` of the portal at `host`
/// (as `host:port`) with this `Origin`, and its `Access-Control-Allow-Origin`.
/// An `OPTIONS` is a CORS preflight for a JSON `PUT`, as the TV app's
/// commands send.
fn state_with_origin(method: &str, host: &str, origin: Option<&str>) -> (String, Option<String>) {
    let mut socket = TcpStream::connect(host).unwrap();
    socket.set_read_timeout(Some(EXIT)).unwrap();
    let origin = origin.map_or(String::new(), |origin| format!("Origin: {origin}\r\n"));
    let preflight = if method == "OPTIONS" {
        "Access-Control-Request-Method: PUT\r\nAccess-Control-Request-Headers: content-type\r\n"
    } else {
        ""
    };
    write!(
        socket,
        "{method} /api/v1/state HTTP/1.1\r\nHost: {host}\r\n{origin}{preflight}Connection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).unwrap();
    let mut lines = response.lines();
    let status = lines.next().unwrap_or_default().to_owned();
    let allowed = lines.take_while(|line| !line.is_empty()).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("access-control-allow-origin")
            .then(|| value.trim().to_owned())
    });
    (status, allowed)
}

/// The TV app is a `file://` page (`Origin: null`) and gets CORS; a phone's
/// browser opens the portal itself (same origin); a page of any other site
/// is refused.
#[test]
fn the_portal_serves_the_tv_app_and_its_own_pages_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = spawn(&dir.path().join("settings.json"));
    let mut events = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    events.read_line(&mut line).unwrap();
    let event: serde_json::Value = serde_json::from_str(&line).unwrap();
    let portal = event["portal"].as_str().unwrap();
    let host = portal
        .strip_prefix("http://")
        .unwrap()
        .trim_end_matches('/')
        .to_owned();

    let served = |(status, _): &(String, Option<String>)| status.starts_with("HTTP/1.1 200");
    let tv_app = state_with_origin("GET", &host, Some("null"));
    assert!(served(&tv_app), "{tv_app:?}");
    assert_eq!(tv_app.1.as_deref(), Some("null"));
    let preflight = state_with_origin("OPTIONS", &host, Some("null"));
    assert!(preflight.0.starts_with("HTTP/1.1 204"), "{preflight:?}");
    assert_eq!(preflight.1.as_deref(), Some("null"));
    let phone = state_with_origin("GET", &host, Some(&format!("http://{host}")));
    assert!(served(&phone), "{phone:?}");
    assert_eq!(phone.1, None, "the same origin needs no CORS");
    let curl = state_with_origin("GET", &host, None);
    assert!(served(&curl), "{curl:?}");
    for method in ["GET", "OPTIONS"] {
        let other = state_with_origin(method, &host, Some("http://ads.example"));
        assert!(other.0.starts_with("HTTP/1.1 403"), "{method}: {other:?}");
        assert_eq!(other.1, None);
    }

    drop(child.stdin.take());
    assert_eq!(
        wait_child(&mut child, EXIT)
            .expect("the worker exits")
            .code(),
        Some(0)
    );
}
