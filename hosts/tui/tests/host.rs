//! The built `prysm-tui` binary without a terminal: the preview fails cleanly
//! while lighting and the portal keep running.
#[path = "../../common/tests/launch/mod.rs"]
mod launch;

use launch::{Host, check_arguments, check_settings, get, null_settings, put_target};
use prysm_capture_device::DeviceSource;
use prysm_capture_file::FileSource;
use prysm_capture_remote::RemoteReceiver;
use prysm_output_wled::Wled;
use prysm_ports::{Registry, SettingsStore};
use prysm_runtime::NullOutput;
use prysm_settings_file::SettingsFile;

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

#[test]
fn example_settings_load_and_validate() {
    let mut registry = Registry::default();
    registry.add_source(DeviceSource);
    registry.add_source(FileSource);
    registry.add_source(RemoteReceiver::default());
    registry.add_output(Wled::default());
    registry.add_output(NullOutput);
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/settings.example.json");
    let settings = SettingsFile::new(path).load().unwrap().unwrap();
    registry.validate(&settings).unwrap();
    assert_eq!(settings.output.unwrap().kind, "null");

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/settings.file.example.json");
    let settings = SettingsFile::new(path).load().unwrap().unwrap();
    registry.validate(&settings).unwrap();
    assert_eq!(settings.capture.unwrap().kind, "file");
}
