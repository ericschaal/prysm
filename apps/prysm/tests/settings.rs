//! Exercise first-run configuration through the same binary used by containers.
#![cfg(unix)]
mod common;
use prysm_contract::{RequiredSetting, SettingsView, StateResponse};
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};

fn start(path: &Path, address: std::net::SocketAddr) -> tokio::process::Child {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_prysm"))
        .arg("--config")
        .arg(path)
        .arg("--listen")
        .arg(address.to_string())
        .arg("--allowed-host")
        .arg(address.to_string())
        .env("PRYSM_LISTEN", "invalid-env-shadowed-by-cli")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap()
}

async fn stop(child: &mut tokio::process::Child) {
    assert!(
        tokio::process::Command::new("kill")
            .arg("-TERM")
            .arg(child.id().unwrap().to_string())
            .status()
            .await
            .unwrap()
            .success()
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(8), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}

async fn settings(client: &reqwest::Client, base: &str) -> SettingsView {
    client
        .get(format!("{base}/api/settings"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn save_settings(
    client: &reqwest::Client,
    base: &str,
    view: &SettingsView,
    changes: Value,
) -> SettingsView {
    let mut values = serde_json::to_value(&view.settings).unwrap();
    for (key, value) in changes.as_object().unwrap() {
        if key == "processing" {
            for (key, value) in value.as_object().unwrap() {
                values["processing"][key] = value.clone();
            }
        } else {
            values[key] = value.clone();
        }
    }
    client
        .put(format!("{base}/api/settings"))
        .json(&json!({"expected_revision":view.revision,"settings":values}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn ready(client: &reqwest::Client, base: &str, child: &mut tokio::process::Child) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if client
                .get(format!("{base}/api/settings"))
                .send()
                .await
                .is_ok()
            {
                break;
            }
            assert!(child.try_wait().unwrap().is_none());
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "follow one installation through setup, live edits, reset and restart"
)]
async fn first_run_keeps_capture_endpoint_available_through_setup_and_reset() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    let available = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = available.local_addr().unwrap();
    drop(available);
    let base = format!("http://{address}");
    let mut child = start(&path, address);
    let client = common::http_client(Duration::from_secs(4));
    ready(&client, &base, &mut child).await;
    let initial = settings(&client, &base).await;
    assert_eq!(
        initial.missing_required,
        [RequiredSetting::Input, RequiredSetting::Output]
    );
    assert!(
        !path.exists(),
        "opening setup must not create a settings file"
    );
    assert!(
        client
            .get(format!("{base}/"))
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert_eq!(
        client
            .get(format!("{base}/api/state"))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::CONFLICT
    );
    assert_eq!(
        client
            .get(format!("{base}/api/settings"))
            .header("Host", "attacker.test")
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::FORBIDDEN
    );

    let configured = save_settings(&client, &base, &initial, json!({
        "input":{"type":"webos","capture_resolution":[4,2],"refresh_rate_hz":30},
        "output":{"type":"wled","url":"http://127.0.0.1:1/","leds":{"top":1,"right":1,"bottom":1,"left":1}}
    })).await;
    assert_eq!(configured.missing_required, Vec::<RequiredSetting>::new());
    let state: StateResponse = client
        .get(format!("{base}/api/state"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(state.config_revision, configured.revision);
    assert_eq!(state.pipeline.phase, prysm_contract::PipelinePhase::Idle);
    assert_eq!(state.pipeline.frames_processed, 0);
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(
        saved.get("web").is_none(),
        "deployment options must not be saved"
    );
    assert_eq!(client.put(format!("{base}/api/settings"))
        .json(&json!({"expected_revision":configured.revision,"settings":{"web":{"listen":"127.0.0.1:9999"}}}))
        .send().await.unwrap().status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        client
            .put(format!("{base}/api/settings"))
            .json(&json!({"expected_revision":initial.revision,"settings":configured.settings}))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::CONFLICT
    );

    let repaired = save_settings(
        &client,
        &base,
        &configured,
        json!({"processing":{"brightness_percent":42}}),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state: StateResponse = client
                .get(format!("{base}/api/state"))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if matches!(
                state.operation,
                prysm_contract::OperationStatus::Succeeded { .. }
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        tokio_tungstenite::connect_async(format!("ws://{address}/api/capture"))
            .await
            .is_ok()
    );
    let incomplete = save_settings(&client, &base, &repaired, json!({"input":null})).await;
    assert_eq!(incomplete.missing_required, [RequiredSetting::Input]);
    assert!(
        tokio_tungstenite::connect_async(format!("ws://{address}/api/capture"))
            .await
            .is_ok()
    );
    stop(&mut child).await;

    let available = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = available.local_addr().unwrap();
    drop(available);
    let base = format!("http://{address}");
    let mut child = start(&path, address);
    ready(&client, &base, &mut child).await;
    let restored = settings(&client, &base).await;
    assert_eq!(restored.missing_required, [RequiredSetting::Input]);
    assert_eq!(restored.settings.processing.brightness_percent.get(), 42);
    assert_ne!(restored.revision, incomplete.revision);
    stop(&mut child).await;
}

#[tokio::test]
async fn https_wled_observer_survives_a_failed_tls_handshake() {
    use tokio::io::AsyncReadExt as _;
    let wled = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config: Value = serde_json::from_str(include_str!("fixtures/config.json")).unwrap();
    config.as_object_mut().unwrap().remove("revision");
    config["calibration"] = json!({"profiles": [], "active": null});
    config["output"]["url"] = format!("https://{}/", wled.local_addr().unwrap()).into();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let mut child = start(&path, "127.0.0.1:0".parse().unwrap());

    // Both the HTTP poll and the WebSocket observer must start TLS, even if the peer then closes.
    tokio::time::timeout(Duration::from_secs(5), async {
        for _ in 0..2 {
            let (mut stream, _) = wled.accept().await.unwrap();
            assert_eq!(stream.read_u8().await.unwrap(), 0x16);
        }
    })
    .await
    .unwrap();
    stop(&mut child).await;
}

#[test]
fn check_does_not_initialize_missing_settings() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_prysm"))
        .arg("--config")
        .arg(&path)
        .arg("check")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("settings are incomplete"));
    assert!(!path.exists());
}
