//! The built `prysm-tui` binary without a terminal: the preview fails cleanly
//! while lighting and the portal keep running.

use prysm_capture_device::DeviceSource;
use prysm_ports::SettingsStore;
use prysm_runtime::Prysm;
use prysm_settings_file::SettingsFile;
use prysm_testkit::host::{Host, check_arguments, check_settings, get, null_settings, put_target};

const BINARY: &str = env!("CARGO_BIN_EXE_prysm-tui");

#[test]
fn help_and_bad_arguments() {
    check_arguments(BINARY, "prysm-tui");
}

#[test]
fn check_settings_validates_and_exits() {
    check_settings(BINARY, "prysm-tui");
}

#[test]
fn without_a_terminal_lighting_and_portal_keep_running_until_sigterm() {
    let dir = tempfile::tempdir().unwrap();
    let settings = null_settings(dir.path());
    let mut host = Host::spawn(
        BINARY,
        &[settings.to_str().unwrap(), "--listen", "127.0.0.1:0"],
    );
    let port = host.portal_port();
    host.wait_for("target off, session stopped");
    host.wait_for("surface failed, lighting continues: the terminal surface requires");

    let (status, body) = get(port, "/api/v1/state");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""target":"off""#), "{body}");
    assert!(body.contains("terminal surface"), "{body}");
    assert_eq!(
        put_target(port, "off", Some("http://evil.example")).status,
        403
    );
    assert_eq!(put_target(port, "off", None).status, 204);

    let (status, stderr) = host.terminate();
    assert!(status.success(), "{stderr:#?}");
}

/// Validated with the adapters the hosts register.
#[test]
fn example_settings_load_and_validate() {
    let check = |path: &str| {
        let (adapters, _) = prysm_host::desktop_adapters(DeviceSource);
        adapters(Prysm::builder(SettingsFile::new(path)))
            .check()
            .unwrap();
        SettingsFile::new(path).load().unwrap().unwrap()
    };
    let settings = check(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/settings.example.json"
    ));
    assert_eq!(settings.output.unwrap().kind, "null");
    let settings = check(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/settings.file.example.json"
    ));
    assert_eq!(settings.capture.unwrap().kind, "file");
}
