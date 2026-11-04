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
        app.request(
            AppRequest::SetConfig(Box::new(app.state.config.clone())),
            now
        )
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
        app.request(AppRequest::SetConfig(Box::new(changed)), Instant::now())
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
        app.request(AppRequest::SetConfig(Box::new(changed)), Instant::now())
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
    app.request(AppRequest::SetConfig(Box::new(changed)), now)
        .unwrap();
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
    echo 'Connection refused' >&2; exit 1
fi
trap 'exit 0' TERM
while :; do sleep 0.01; done
"#,
    );
    app.power(Some("Active".into()), None, now);
    operate(&mut app, AppRequest::Start, now).unwrap();
    let mut next = old.clone();
    next.instance_url = Some("http://unavailable".parse().unwrap());
    let refused = operate(&mut app, AppRequest::SetConfig(Box::new(next.clone())), now).unwrap();
    assert_eq!(
        refused,
        serde_json::json!({"returnValue": false, "errorText": "Could not verify Prysm: Connection refused"})
    );
    assert_eq!(app.files.load().unwrap(), old);
    assert!(app.state.enabled && app.state.running);
    next.instance_url = Some("https://prysm.example".parse().unwrap());
    let saved = operate(&mut app, AppRequest::SetConfig(Box::new(next.clone())), now).unwrap();
    assert_eq!(saved["config"]["instanceUrl"], "https://prysm.example");
    assert_eq!(app.files.load().unwrap(), next);
    assert!(app.state.running);
    operate(&mut app, AppRequest::Stop, now).unwrap();
}

#[test]
fn reopening_repairs_an_interrupted_autostart_change() {
    let dir = sandbox();
    let files = Files::sandbox(dir.path()).unwrap();
    let mut config = Config {
        instance_url: Some("https://prysm.example".parse().unwrap()),
        ..Config::default()
    };
    let hook = dir.path().join("boot-hook");
    for enabled in [false, true] {
        config.autostart = enabled;
        files.persist(&config).unwrap();
        files.autostart(!enabled).unwrap();
        let app = App::new(Files::sandbox(dir.path()).unwrap()).unwrap();
        assert_eq!(app.state.enabled, enabled);
        assert_eq!(hook.exists(), enabled);
    }
    fs::write(&hook, "unrelated").unwrap();
    let app = App::new(files).unwrap();
    assert!(app.state.error.is_some());
    assert_eq!(fs::read_to_string(hook).unwrap(), "unrelated");
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

#[test]
fn verified_receiver_save_failure_replies_without_disrupting_the_incumbent() {
    let dir = sandbox();
    let mut app = app(&dir);
    script(
        &app.files.worker,
        r#"
if [ "$1" = --verify-instance ]; then
    while [ ! -f "$0.verified" ]; do sleep 0.01; done
    exit 0
fi
trap 'exit 0' TERM
while :; do sleep 0.01; done
"#,
    );
    let now = Instant::now();
    app.power(Some("Active".into()), None, now);
    operate(&mut app, AppRequest::Start, now).unwrap();
    let original = app.state.config.clone();
    let pid = app.worker.as_ref().unwrap().child_id();
    let next = Config {
        instance_url: Some("https://new.example".parse().unwrap()),
        ..original.clone()
    };
    assert!(
        app.request(AppRequest::SetConfig(Box::new(next)), now)
            .unwrap()
            .is_none()
    );
    fs::write(dir.path().join("boot-hook"), "another owner's hook").unwrap();
    fs::write(dir.path().join("worker.verified"), "").unwrap();
    let reply = completion(&mut app, now);
    assert_eq!(reply["returnValue"], false);
    assert!(reply["errorText"].as_str().unwrap().contains("boot hook"));
    assert!(app.pending.is_none());
    assert_eq!(app.files.load().unwrap(), original);
    assert_eq!(app.worker.as_ref().unwrap().child_id(), pid);
    assert!(app.state.enabled && !app.state.stopping);
    operate(&mut app, AppRequest::Stop, now).unwrap();
}

#[test]
#[cfg(feature = "embedded-prysm")]
fn switching_waits_for_reap_and_the_embedded_worker_uses_only_local_startup() {
    use crate::config::ProcessingLocation;
    let dir = sandbox();
    let mut app = app(&dir);
    let now = Instant::now();
    script(
        &app.files.worker,
        r#"
if [ "$1" = --verify-instance ]; then echo "receiver offline" >&2; exit 1; fi
trap '' TERM
printf '%s\n' '{"type":"connected"}' >&"$PRYSM_EVENTS_FD"
while :; do sleep 0.01; done
"#,
    );
    script(
        &dir.path().join("prysm-webos-local"),
        r#"
test "$1" = "--supervised" || exit 2
test "$2" = "$(dirname "$0")/portal-settings.json" || exit 2
echo "$PRYSM_WORKER_CONFIG" > "$0.startup"
printf '%s\n' '{"type":"portal_ready","address":"192.0.2.1:8787"}' >&"$PRYSM_EVENTS_FD"
while IFS= read -r -n 1 command; do
  case "$command" in 0) event=capture_idle ;; p) event=capture_paused ;; *) event=capturing ;; esac
  printf '{"type":"%s"}\n' "$event" >&"$PRYSM_EVENTS_FD"
done
"#,
    );
    app.power(Some("Active".into()), None, now);
    operate(&mut app, AppRequest::Start, now).unwrap();
    let wait = |app: &mut App, now, done: fn(&App) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !done(app) {
            app.tick(now);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    wait(&mut app, now, |a| a.state.contract().receiver_connected);
    let remote_pid = app.worker.as_ref().unwrap().child_id();
    let remote = app.state.config.clone();
    let embedded = Config {
        location: ProcessingLocation::Embedded,
        ..remote.clone()
    };
    let switched = operate(
        &mut app,
        AppRequest::SetConfig(Box::new(embedded.clone())),
        now,
    )
    .unwrap();
    assert_eq!(switched["config"]["location"], "embedded");
    assert_eq!(switched["stopping"], true);
    assert_eq!(switched["receiverConnected"], false);
    app.video_available(VideoAvailability::Available);
    app.tick(now + Duration::from_millis(4999));
    assert_eq!(app.worker.as_ref().unwrap().child_id(), remote_pid);
    assert!(!dir.path().join("prysm-webos-local.startup").exists());
    wait(&mut app, now + Duration::from_secs(5), |a| {
        a.state.contract().capturing
    });
    let local_pid = app.worker.as_ref().unwrap().child_id();
    assert_ne!(remote_pid, local_pid);
    let startup: Value =
        serde_json::from_slice(&fs::read(dir.path().join("prysm-webos-local.startup")).unwrap())
            .unwrap();
    assert!(startup.get("output").is_none());
    assert!(startup.get("compression").is_none() && startup.get("instanceUrl").is_none());
    assert!(!app.state.contract().receiver_connected);
    // The remote URL is unchanged but inactive. Require a successful probe before switching back.
    let refused = operate(
        &mut app,
        AppRequest::SetConfig(Box::new(remote)),
        now + Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(refused["returnValue"], false);
    assert_eq!(app.worker.as_ref().unwrap().child_id(), local_pid);
    assert_eq!(app.files.load().unwrap(), embedded);
    assert!(app.state.contract().capturing && !app.state.stopping);
    app.video_available(VideoAvailability::Unavailable);
    wait(&mut app, now + Duration::from_secs(5), |a| {
        !a.state.contract().capturing
    });
    operate(&mut app, AppRequest::Stop, now + Duration::from_secs(5)).unwrap();
}

#[test]
#[cfg(feature = "embedded-prysm")]
fn choosing_embedded_starts_an_idle_portal_and_cleanup_cannot_relaunch_it() {
    use crate::config::ProcessingLocation;
    fn until(app: &mut App, now: Instant, ready: impl Fn(&Supervisor) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !ready(&app.state) {
            app.tick(now);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let dir = sandbox();
    let files = Files::sandbox(dir.path()).unwrap();
    script(&files.scanner, "exit 0");
    script(
        &dir.path().join("prysm-webos-local"),
        r#"
printf '%s\n' '{"type":"portal_ready","address":"192.0.2.5:8787"}' >&"$PRYSM_EVENTS_FD"
while IFS= read -r -n 1 command; do
  case "$command" in 0) event=capture_idle ;; p) event=capture_paused ;; *) event=capturing ;; esac
  printf '{"type":"%s"}\n' "$event" >&"$PRYSM_EVENTS_FD"
done
"#,
    );
    let mut app = App::new(files).unwrap();
    let now = Instant::now();
    app.power(Some("Active".into()), None, now);
    let config = Config {
        location: ProcessingLocation::Embedded,
        ..Config::default()
    };
    operate(&mut app, AppRequest::SetConfig(Box::new(config)), now).unwrap();
    until(&mut app, now, |s| s.contract().portal_url.is_some());
    let pid = app.worker.as_ref().unwrap().child_id();
    assert!(app.state.enabled && !app.state.contract().capturing);
    assert!(dir.path().join("boot-hook").exists());
    // Stop also needs an acknowledgement before the very first captured frame.
    assert!(app.request(AppRequest::Stop, now).unwrap().is_none());
    completion(&mut app, now);
    operate(&mut app, AppRequest::Start, now).unwrap();
    app.video_available(VideoAvailability::Available);
    until(&mut app, now, |s| s.contract().capturing);
    let paused = operate(&mut app, AppRequest::Stop, now).unwrap();
    assert_eq!(paused["portalUrl"], "http://192.0.2.5:8787/");
    assert_eq!(paused["capturing"], false);
    assert_eq!(paused["enabled"], false);
    assert_eq!(app.worker.as_ref().unwrap().child_id(), pid);
    operate(&mut app, AppRequest::Start, now).unwrap();
    until(&mut app, now, |s| s.contract().capturing);
    app.power(Some("Screen Saver".into()), None, now);
    until(&mut app, now, |s| !s.contract().capturing);
    assert_eq!(app.worker.as_ref().unwrap().child_id(), pid);
    assert!(app.state.contract().portal_url.is_some());
    let cleaned = operate(&mut app, AppRequest::Cleanup, now).unwrap();
    assert_eq!(cleaned["senderRunning"], false);
    assert!(cleaned.get("portalUrl").is_none());
    assert!(!dir.path().join("boot-hook").exists());
    let mut reopened = App::new(Files::sandbox(dir.path()).unwrap()).unwrap();
    reopened.power(Some("Active".into()), None, now);
    reopened.tick(now);
    assert!(reopened.worker.is_none());
    assert_eq!(reopened.state.config.location, ProcessingLocation::Remote);
    assert!(reopened.state.config.instance_url.is_none());
}

#[test]
#[cfg(feature = "embedded-prysm")]
fn leaving_embedded_restores_the_disabled_remote_selection() {
    let now = Instant::now();
    let remote = Config {
        instance_url: Some("http://remote.test".parse().unwrap()),
        ..Config::default()
    };
    let mut state = Supervisor::new(remote.clone());
    assert!(!state.enabled);
    state.configure(
        Config {
            location: crate::config::ProcessingLocation::Embedded,
            ..remote.clone()
        }
        .try_into()
        .unwrap(),
        now,
    );
    assert!(state.enabled);
    state.configure(remote.try_into().unwrap(), now);
    assert!(!state.enabled);
}
