//! Private files and real child processes, with virtual deadlines; no TV required.
use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

pub(crate) fn sandbox() -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("prysm-supervisor.")
        .tempdir()
        .unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    dir
}
pub(crate) fn script(path: &std::path::Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
fn app(dir: &tempfile::TempDir) -> App {
    let files = Files::sandbox(dir.path()).unwrap();
    files
        .persist(&Config {
            receiver: Some("127.0.0.1:19400".parse().unwrap()),
            ..Config::default()
        })
        .unwrap();
    script(&files.scanner, "exit 0");
    script(
        &files.worker,
        "trap 'exit 0' TERM; printf '%s\\n' '{\"type\":\"connected\"}' >&\"$PRYSM_EVENTS_FD\"; while :; do sleep 0.01; done",
    );
    App::new(files).unwrap()
}
fn request(app: &mut App, method: &str, value: Value, now: Instant) -> Result<Option<Value>> {
    app.request(method, value, "io.prysm.capture.probe.app", now)
}
fn operate(app: &mut App, method: &str, value: Value, now: Instant) -> Result<Value> {
    Ok(match request(app, method, value, now)? {
        Some(value) => value,
        None => completion(app, now),
    })
}
fn completion(app: &mut App, now: Instant) -> Value {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(value) = app.tick(now) {
            return value;
        }
        assert!(Instant::now() < deadline, "operation did not complete");
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn start_stop_cleanup_serialize_and_preserve_settings() {
    let dir = sandbox();
    let mut app = app(&dir);
    let now = Instant::now();
    app.power(Some("Active".into()), None, now);
    assert!(
        app.request(
            "start",
            json!({"sender":"io.prysm.capture.probe.app"}),
            "untrusted",
            now
        )
        .is_err()
    );
    assert!(!app.state.enabled);
    script(
        &app.files.scanner,
        "while [ ! -f \"$0.ready\" ]; do sleep 0.01; done",
    );
    assert!(
        request(&mut app, "start", json!({}), now)
            .unwrap()
            .is_none()
    );
    assert!(request(&mut app, "setConfig", json!({}), now).is_err());
    fs::write(dir.path().join("scan-services.ready"), "").unwrap();
    assert_eq!(completion(&mut app, now)["senderRunning"], true);
    let pid = app.worker.as_ref().unwrap().child_id();
    script(&app.files.scanner, "exit 7");
    assert!(match operate(&mut app, "start", json!({}), now) {
        Ok(value) => value["returnValue"] == false,
        Err(_) => true,
    });
    assert_eq!(app.worker.as_ref().unwrap().child_id(), pid);
    assert!(!app.state.stopping);
    script(&app.files.scanner, "exit 0");
    assert_eq!(
        operate(&mut app, "stop", json!({}), now).unwrap()["senderRunning"],
        false
    );
    app.power(Some("Suspend".into()), None, now);
    app.power(Some("Active".into()), None, now);
    app.tick(now);
    assert!(app.worker.is_none());
    assert_eq!(
        operate(&mut app, "cleanup", json!({}), now).unwrap()["config"]["autostart"],
        false
    );
    assert!(!dir.path().join("capture.manifest.json").exists());
    assert_eq!(
        app.files.load().unwrap().receiver,
        Some("127.0.0.1:19400".parse().unwrap())
    );
}
#[test]
fn hook_failure_rolls_back_config() {
    let dir = sandbox();
    let mut app = app(&dir);
    let original = app.state.config.clone();
    fs::write(dir.path().join("boot-hook"), "unrelated").unwrap();
    let mut changed = original.clone();
    changed.receiver = Some("[::1]:42".parse().unwrap());
    changed.autostart = true;
    assert!(
        request(
            &mut app,
            "setConfig",
            serde_json::to_value(changed).unwrap(),
            Instant::now()
        )
        .is_err()
    );
    assert_eq!(app.state.config, original);
    assert_eq!(app.files.load().unwrap(), original);
    assert!(app.pending.is_none());
    assert_eq!(
        fs::read_to_string(dir.path().join("boot-hook")).unwrap(),
        "unrelated"
    );
}
#[test]
fn scanner_timeout_is_reaped_and_shutdown_cancels_pending_start() {
    let dir = sandbox();
    let mut app = app(&dir);
    let now = Instant::now();
    script(&app.files.scanner, "exec sleep 30");
    request(&mut app, "start", json!({}), now).unwrap();
    assert_eq!(
        completion(&mut app, now + Duration::from_secs(10))["returnValue"],
        false
    );
    assert!(!app.state.running);
    script(&app.files.scanner, "exec sleep 30");
    request(&mut app, "start", json!({}), now).unwrap();
    app.shutdown(now);
    assert_eq!(completion(&mut app, now)["returnValue"], false);
    assert!(app.stopped());
}
#[test]
fn config_restart_waits_for_kill_and_reap() {
    let dir = sandbox();
    let mut app = app(&dir);
    script(
        &app.files.worker,
        "trap '' TERM; printf '%s\\n' '{\"type\":\"connected\"}' >&\"$PRYSM_EVENTS_FD\"; while :; do sleep 0.01; done",
    );
    let now = Instant::now();
    app.power(Some("Active".into()), None, now);
    operate(&mut app, "start", json!({}), now).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while app.state.snapshot()["receiverConnected"] != true {
        app.tick(now);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let pid = app.worker.as_ref().unwrap().child_id();
    let mut changed = app.state.config.clone();
    changed.receiver = Some("[::1]:42".parse().unwrap());
    request(
        &mut app,
        "setConfig",
        serde_json::to_value(changed).unwrap(),
        now,
    )
    .unwrap();
    app.tick(now + Duration::from_millis(4999));
    assert_eq!(app.worker.as_ref().unwrap().child_id(), pid);
    while app.worker.as_ref().is_some_and(|w| w.child_id() == pid) {
        app.tick(now + Duration::from_secs(5));
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(app.worker.is_some());
    app.shutdown(now);
    while !app.stopped() {
        app.tick(now + Duration::from_secs(5));
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn readiness_loss_is_latched_across_rapid_input_transitions() {
    let dir = sandbox();
    let mut app = app(&dir);
    assert_eq!(app.video.availability, VideoAvailability::Unavailable);
    app.video_available(VideoAvailability::Available);
    assert!(!app.video.reset);
    app.video_available(VideoAvailability::Unavailable);
    app.video_available(VideoAvailability::Available);
    assert!(app.video.reset);
    assert_eq!(app.video.availability, VideoAvailability::Available);
    app.video.reset = false;
    app.video_available(VideoAvailability::Unknown);
    assert!(!app.video.reset);
}
