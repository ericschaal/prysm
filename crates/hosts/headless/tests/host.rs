//! The built `prysm-headless` binary: arguments, idle start, portal,
//! SIGTERM. Also the adapters the desktop hosts register
//! (`prysm_host::desktop_adapters`) against the example settings and the
//! contract's adapter fixtures.

use prysm_capture_device::DeviceSource;
use prysm_ports::{Settings, SettingsStore};
use prysm_runtime::Prysm;
use prysm_settings_file::SettingsFile;
use prysm_testkit::host::{
    Host, check_arguments, check_settings, get, null_settings, put_target, request,
};
use prysm_testkit::host_adapter_settings;
use std::io::Write;
use std::net::TcpStream;
use std::process::Command;
use std::time::{Duration, Instant};

const BINARY: &str = env!("CARGO_BIN_EXE_prysm-headless");

#[test]
fn help_and_bad_arguments() {
    check_arguments(BINARY, "prysm-headless");
}

#[test]
fn check_settings_validates_and_exits() {
    check_settings(BINARY, "prysm-headless");
}

#[test]
fn starts_idle_serves_the_portal_without_a_token_and_stops_on_sigterm() {
    let dir = tempfile::tempdir().unwrap();
    let settings = null_settings(dir.path());
    let mut host = Host::spawn(
        BINARY,
        &[settings.to_str().unwrap(), "--listen", "127.0.0.1:0"],
    );
    let port = host.portal_port();
    assert!(
        host.wait_for("portal http://")
            .contains("other sites' pages refused")
    );
    host.wait_for("target off, session stopped");

    let (status, body) = get(port, "/api/v1/state");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""target":"off""#), "{body}");
    assert!(body.contains(r#""kind":"null""#), "{body}");

    let foreign = put_target(port, "off", Some("http://evil.example"));
    assert_eq!(foreign.status, 403, "{}", foreign.body);
    assert!(foreign.body.contains(r#""kind":"forbidden""#));
    assert_eq!(foreign.header("access-control-allow-origin"), None);
    let own = format!("http://127.0.0.1:{port}");
    assert_eq!(put_target(port, "off", Some(&own)).status, 204);
    assert_eq!(put_target(port, "off", None).status, 204);

    let (status, stderr) = host.terminate();
    assert!(status.success(), "{stderr:#?}");
    assert!(stderr.iter().any(|line| line.ends_with("stopped")));
    assert!(!dir.path().join("portal.token").exists());
}

/// The TV app (a packaged webOS app, `Origin: null`) may control a desktop
/// Prysm started with `--allow-origin null`; other sites still may not.
#[test]
fn the_null_origin_is_allowed_only_when_listed() {
    let dir = tempfile::tempdir().unwrap();
    let settings = null_settings(dir.path());
    let mut host = Host::spawn(
        BINARY,
        &[
            settings.to_str().unwrap(),
            "--listen",
            "127.0.0.1:0",
            "--allow-origin",
            "null",
        ],
    );
    let port = host.portal_port();
    let put = put_target(port, "off", Some("null"));
    assert_eq!(put.status, 204, "{}", put.body);
    assert_eq!(put.header("access-control-allow-origin"), Some("null"));
    assert_eq!(
        put_target(port, "off", Some("http://evil.example")).status,
        403
    );
    let (status, stderr) = host.terminate();
    assert!(status.success(), "{stderr:#?}");
}

#[test]
fn allowed_origins_get_cors() {
    let dir = tempfile::tempdir().unwrap();
    let settings = null_settings(dir.path());
    let tv = "http://192.168.1.30:3000";
    let mut host = Host::spawn(
        BINARY,
        &[
            settings.to_str().unwrap(),
            "--listen",
            "127.0.0.1:0",
            "--allow-origin",
            tv,
        ],
    );
    let port = host.portal_port();
    assert!(host.wait_for("portal http://").contains(tv));

    let preflight = request(
        port,
        "OPTIONS",
        "/api/v1/target",
        &[
            ("Origin", tv),
            ("Access-Control-Request-Method", "PUT"),
            ("Access-Control-Request-Headers", "content-type"),
        ],
        "",
    );
    assert_eq!(preflight.status, 204, "{}", preflight.headers);
    assert_eq!(preflight.header("access-control-allow-origin"), Some(tv));
    assert_eq!(
        preflight.header("access-control-allow-methods"),
        Some("GET, POST, PUT, DELETE")
    );
    let put = put_target(port, "off", Some(tv));
    assert_eq!(put.status, 204, "{}", put.body);
    assert_eq!(put.header("access-control-allow-origin"), Some(tv));
    assert_eq!(put.header("vary"), Some("origin"));
    assert_eq!(
        put_target(port, "off", Some("http://192.168.1.31:3000")).status,
        403
    );

    let (status, stderr) = host.terminate();
    assert!(status.success(), "{stderr:#?}");
}

/// A second SIGTERM while shutdown waits (here for a request whose body
/// never completes) exits at once with its own code.
#[test]
fn a_second_signal_during_shutdown_exits_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let settings = null_settings(dir.path());
    let mut host = Host::spawn(
        BINARY,
        &[settings.to_str().unwrap(), "--listen", "127.0.0.1:0"],
    );
    let port = host.portal_port();
    host.wait_for("target off, session stopped");
    // The portal lets it finish for up to 5 s once shutdown starts.
    let mut unfinished = TcpStream::connect(("127.0.0.1", port)).unwrap();
    unfinished
        .write_all(
            b"PUT /api/v1/settings HTTP/1.1\r\nHost: 127.0.0.1\r\n\
              Content-Type: application/json\r\nContent-Length: 1000\r\n\r\n{",
        )
        .unwrap();
    let first = Command::new("kill")
        .args(["-TERM", &host.pid().to_string()])
        .status()
        .unwrap();
    assert!(first.success());
    host.wait_for("SIGTERM, shutting down; a second SIGINT or SIGTERM exits at once");
    let second = Instant::now();
    let (status, stderr) = host.terminate();
    assert_eq!(status.code(), Some(130), "{stderr:#?}");
    assert!(second.elapsed() < Duration::from_secs(3), "{stderr:#?}");
    assert!(
        stderr
            .iter()
            .any(|line| line == "prysm-headless: SIGTERM again, exited without finishing shutdown"),
        "{stderr:#?}"
    );
    assert!(!stderr.iter().any(|line| line == "prysm-headless: stopped"));
    drop(unfinished);
}

/// Validated with the adapters the hosts register.
#[test]
fn example_settings_load_and_validate() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/settings.example.json");
    let (adapters, _) = prysm_host::desktop_adapters(DeviceSource);
    adapters(Prysm::builder(SettingsFile::new(path)))
        .check()
        .unwrap();
    let settings = SettingsFile::new(path).load().unwrap().unwrap();
    assert_eq!(settings.capture.unwrap().kind, "device");
    assert_eq!(settings.output.unwrap().kind, "wled");
}

/// Every adapter section of the `host-desktop` contract decodes with the
/// adapters the desktop hosts register: a kind they lack fails.
#[test]
fn contract_adapter_fixtures_decode_with_the_registered_adapters() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    for fixture in host_adapter_settings("host-desktop") {
        let settings: Settings = serde_json::from_value(fixture.clone()).unwrap();
        SettingsFile::new(&path).save(&settings).unwrap();
        let (adapters, _) = prysm_host::desktop_adapters(DeviceSource);
        adapters(Prysm::builder(SettingsFile::new(&path)))
            .check()
            .unwrap_or_else(|error| panic!("{fixture}: {error}"));
    }
}
