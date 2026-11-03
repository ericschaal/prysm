#[test]
fn check_and_startup_reject_unknown_output() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    std::fs::write(&path, r#"{"output":{"type":"unknown"}}"#).unwrap();
    for check in [false, true] {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_prysm"));
        command.arg("--config").arg(&path);
        if check {
            command.arg("check");
        }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("unknown variant `unknown`"), "{error}");
        assert!(error.contains("expected `wled`"), "{error}");
    }
}

#[test]
fn invalid_saved_settings_are_reported_and_never_replaced() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    for invalid in [
        "{broken json",
        r#"{"web":{"allowed_hosts":[]}}"#,
        r#"{"input":{"type":"device","device":"/dev/video0","capture_resolution":[4,4],"refresh_rate_hz":0}}"#,
        r#"{"output":{"type":"wled","url":"file:///tmp/wled","leds":{"top":1,"right":0,"bottom":0,"left":0}}}"#,
    ] {
        std::fs::write(&path, invalid).unwrap();
        for check in [false, true] {
            let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_prysm"));
            command.arg("--config").arg(&path);
            if check {
                command.arg("check");
            }
            let output = command.output().unwrap();
            assert!(!output.status.success());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), invalid);
            assert!(String::from_utf8_lossy(&output.stderr).contains("Error:"));
        }
    }
}
