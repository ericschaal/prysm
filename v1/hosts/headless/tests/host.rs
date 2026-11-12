//! The built `prysm-headless` binary: arguments, idle start, portal, SIGTERM.
#[path = "../../common/tests/launch/mod.rs"]
mod launch;

use launch::{Host, check_arguments, get, null_settings, put_target, request};
use prysm_capture_device::DeviceSource;
use prysm_capture_remote::RemoteReceiver;
use prysm_output_wled::Wled;
use prysm_ports::{Registry, SettingsStore};
use prysm_runtime::NullOutput;
use prysm_settings_file::FileSettings;

const BINARY: &str = env!("CARGO_BIN_EXE_prysm-headless");

#[test]
fn help_and_bad_arguments() {
    check_arguments(BINARY, "prysm-headless");
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
        Some("GET, PUT")
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

#[test]
fn example_settings_load_and_validate() {
    let mut registry = Registry::default();
    registry.add_source(DeviceSource);
    registry.add_source(RemoteReceiver);
    registry.add_output(Wled::default());
    registry.add_output(NullOutput);
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/settings.example.json");
    let settings = FileSettings::new(path).load().unwrap().unwrap();
    registry.validate(&settings).unwrap();
    assert_eq!(settings.capture.unwrap().kind, "device");
    assert_eq!(settings.output.unwrap().kind, "wled");
}
