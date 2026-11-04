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
    // dash rejects the multi-digit event descriptors produced by parallel tests.
    fs::write(path, format!("#!/bin/bash\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
pub(crate) fn test_process() -> std::process::Command {
    let mut command = std::process::Command::new("sh");
    command.args(["-c", "ulimit -c 0; exec \"$@\"", "test"]);
    // Cargo's runner is not automatically applied to child processes.
    if let Ok(runner) = std::env::var("CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABI_RUNNER") {
        command.args(runner.split_whitespace());
    }
    command.arg(std::env::current_exe().unwrap());
    command
}
fn app(dir: &tempfile::TempDir) -> App {
    let files = Files::sandbox(dir.path()).unwrap();
    files
        .persist(&Config {
            instance_url: Some("http://127.0.0.1:8787".parse().unwrap()),
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
fn operate(app: &mut App, request: AppRequest, now: Instant) -> Result<Value> {
    Ok(match app.request(request, now)? {
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
    assert!(app.authorize("untrusted").is_err());
    assert!(!app.state.enabled);
    script(
        &app.files.scanner,
        "while [ ! -f \"$0.ready\" ]; do sleep 0.01; done",
    );
    assert!(app.request(AppRequest::Start, now).unwrap().is_none());
    assert!(
        app.request(AppRequest::SetConfig(app.state.config.clone()), now)
            .is_err()
    );
    fs::write(dir.path().join("scan-services.ready"), "").unwrap();
    assert_eq!(completion(&mut app, now)["senderRunning"], true);
    let pid = app.worker.as_ref().unwrap().child_id();
    script(&app.files.scanner, "exit 7");
    assert!(match operate(&mut app, AppRequest::Start, now) {
        Ok(value) => value["returnValue"] == false,
        Err(_) => true,
    });
    assert_eq!(app.worker.as_ref().unwrap().child_id(), pid);
    assert!(!app.state.stopping);
    script(&app.files.scanner, "exit 0");
    assert_eq!(
        operate(&mut app, AppRequest::Stop, now).unwrap()["senderRunning"],
        false
    );
    app.power(Some("Suspend".into()), None, now);
    app.power(Some("Active".into()), None, now);
    app.tick(now);
    assert!(app.worker.is_none());
    assert_eq!(
        operate(&mut app, AppRequest::Cleanup, now).unwrap()["config"]["autostart"],
        false
    );
    assert!(!dir.path().join("capture.manifest.json").exists());
    assert_eq!(
        app.files.load().unwrap().instance_url,
        Some("http://127.0.0.1:8787".parse().unwrap())
    );
}
#[test]
fn hook_failure_preserves_the_saved_file() {
    let dir = sandbox();
    let mut app = app(&dir);
    let original = app.state.config.clone();
    let bytes = serde_json::to_vec_pretty(&original).unwrap();
    let path = dir.path().join("config.json");
    fs::write(&path, &bytes).unwrap();
    fs::write(dir.path().join("boot-hook"), "unrelated").unwrap();
    let mut changed = original.clone();
    changed.autostart = true;
    assert!(
        app.request(AppRequest::SetConfig(changed), Instant::now())
            .is_err()
    );
    assert_eq!(app.state.config, original);
    assert_eq!(fs::read(path).unwrap(), bytes);
    assert!(app.pending.is_none());
    assert_eq!(
        fs::read_to_string(dir.path().join("boot-hook")).unwrap(),
        "unrelated"
    );
    let now = Instant::now();
    app.power(Some("Active".into()), None, now);
    operate(&mut app, AppRequest::Start, now).unwrap();
    assert!(app.request(AppRequest::Cleanup, now).is_err());
    assert!(app.state.enabled && app.state.running && !app.state.stopping);
    operate(&mut app, AppRequest::Stop, now).unwrap();
}
#[test]
fn invalid_config_path_leaves_the_boot_hook_untouched() {
    let dir = sandbox();
    let mut app = app(&dir);
    let path = dir.path().join("config.json");
    fs::rename(&path, dir.path().join("saved-config")).unwrap();
    fs::create_dir(path).unwrap();
    let mut changed = app.state.config.clone();
    changed.autostart = true;
    assert!(
        app.request(AppRequest::SetConfig(changed), Instant::now())
            .is_err()
    );
    assert!(!app.state.config.autostart);
    assert!(!dir.path().join("boot-hook").exists());
}
#[test]
fn scanner_timeout_is_reaped_and_shutdown_cancels_pending_start() {
    let dir = sandbox();
    let mut app = app(&dir);
    let now = Instant::now();
    script(&app.files.scanner, "exec sleep 30");
    app.request(AppRequest::Start, now).unwrap();
    assert_eq!(
        completion(&mut app, now + Duration::from_secs(10))["returnValue"],
        false
    );
    assert!(!app.state.running);
    app.request(AppRequest::Start, now).unwrap();
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
    operate(&mut app, AppRequest::Start, now).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while app.state.snapshot()["receiverConnected"] != true {
        app.tick(now);
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let pid = app.worker.as_ref().unwrap().child_id();
    let mut changed = app.state.config.clone();
    changed.metrics = true;
    app.request(AppRequest::SetConfig(changed), now).unwrap();
    app.tick(now + Duration::from_millis(4999));
    assert_eq!(app.worker.as_ref().unwrap().child_id(), pid);
    while app.worker.as_ref().is_some_and(|w| w.child_id() == pid) {
        app.tick(now + Duration::from_secs(5));
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(app.worker.is_some());
    assert!(app.files.load().unwrap().metrics);
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

#[test]
fn changing_instance_verifies_before_save_and_restores_the_previous_sender_on_failure() {
    let dir = sandbox();
    let mut app = app(&dir);
    let now = Instant::now();
    let old = app.state.config.clone();
    script(
        &app.files.worker,
        r#"
if [ "$1" = --verify-instance ]; then
    if [ "$2" = https://prysm.example ]; then exit 0; fi
    echo 'Connection refused' >&2; exit 2
fi
trap 'exit 0' TERM
while :; do sleep 0.01; done
"#,
    );
    app.power(Some("Active".into()), None, now);
    operate(&mut app, AppRequest::Start, now).unwrap();
    let mut next = old.clone();
    next.instance_url = Some("http://unavailable".parse().unwrap());
    let refused = operate(&mut app, AppRequest::SetConfig(next.clone()), now).unwrap();
    assert_eq!(refused["returnValue"], false);
    assert_eq!(refused["retryDefaultPort"], true);
    assert_eq!(app.files.load().unwrap(), old);
    assert!(app.state.enabled && app.state.running);
    next.instance_url = Some("https://prysm.example".parse().unwrap());
    let saved = operate(&mut app, AppRequest::SetConfig(next.clone()), now).unwrap();
    assert_eq!(saved["config"]["instanceUrl"], "https://prysm.example");
    assert_eq!(app.files.load().unwrap(), next);
    assert!(app.state.running);
    operate(&mut app, AppRequest::Stop, now).unwrap();
}

#[test]
fn incompatible_config_is_rejected_without_rewriting_it() {
    let dir = sandbox();
    let path = dir.path().join("config.json");
    let bytes = br#"{"receiver":"192.168.1.10:19400","autostart":true,"source":"video","compression":"none","metrics":true}"#;
    fs::write(&path, bytes).unwrap();
    assert!(App::new(Files::sandbox(dir.path()).unwrap()).is_err());
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn shutdown_waits_for_the_cleanup_rescan() {
    let dir = sandbox();
    let mut app = app(&dir);
    app.files.manifest(true).unwrap();
    script(
        &app.files.scanner,
        "while [ ! -f \"$0.ready\" ]; do sleep 0.01; done; touch \"$0.done\"",
    );
    let now = Instant::now();
    assert!(app.request(AppRequest::Cleanup, now).unwrap().is_none());
    app.shutdown(now);
    assert!(!app.stopped());
    assert!(!dir.path().join("capture.manifest.json").exists());
    fs::write(dir.path().join("scan-services.ready"), "").unwrap();
    assert_eq!(completion(&mut app, now)["enabled"], false);
    assert!(dir.path().join("scan-services.done").exists());
    assert!(app.stopped());
}
