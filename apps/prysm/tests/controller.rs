//! The same controller runs on macOS/Linux; only WLED is an HTTP fixture.
#![cfg(unix)]
use axum::{
    Json, Router,
    routing::{get, post},
};
use prysm_contract::{
    Config, OperationResponse, OperationStatus, Source, StateResponse, Target, UpdateConfig,
};
use std::{
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, Notify};

#[tokio::test]
async fn controller_recovers_from_capture_failure_and_saves_configuration() {
    exercise_controller(None).await;
}

#[tokio::test]
#[ignore = "requires FFmpeg 9+ on PATH"]
async fn file_capture_live_settings_restart_and_shutdown() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("white.mkv");
    assert!(
        tokio::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=white:s=16x16:r=30",
                "-t",
                "20",
                "-c:v",
                "ffv1"
            ])
            .arg(&path)
            .status()
            .await
            .unwrap()
            .success()
    );
    exercise_controller(Some(path)).await;
}

#[allow(
    clippy::too_many_lines,
    reason = "keep each end-to-end controller scenario and its assertions together"
)]
async fn exercise_controller(video: Option<PathBuf>) {
    let wled = Arc::new(Mutex::new(
        serde_json::json!({"state":{"on":false,"lor":2},"info":{"live":false,"lm":null}}),
    ));
    let udp = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let output_address = udp.local_addr().unwrap();
    let packets = Arc::new(AtomicUsize::new(0));
    let packet_count = packets.clone();
    let ddp_state = wled.clone();
    let ddp = tokio::spawn(async move {
        let mut bytes = [0; 1500];
        while let Ok((size, _)) = udp.recv_from(&mut bytes).await {
            packet_count.fetch_add(1, Ordering::Relaxed);
            let mut state = ddp_state.lock().await;
            state["info"]["live"] =
                (state["state"]["on"] == true && state["state"]["lor"] == 0).into();
            state["pixels"] = serde_json::json!(&bytes[10..size]);
        }
    });
    let observed = wled.clone();
    let changed = wled.clone();
    let pause_next = Arc::new(AtomicBool::new(false));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let pause_command = pause_next.clone();
    let command_entered = entered.clone();
    let release_command = release.clone();
    let router = Router::new()
        .route(
            "/json/si",
            get(move || {
                let state = observed.clone();
                async move { Json(state.lock().await.clone()) }
            }),
        )
        .route(
            "/json/state",
            post(move |Json(update): Json<serde_json::Value>| {
                let state = changed.clone();
                let pause = pause_command.swap(false, Ordering::Relaxed);
                let entered = command_entered.clone();
                let release = release_command.clone();
                async move {
                    if pause {
                        entered.notify_one();
                        release.notified().await;
                    }
                    let mut state = state.lock().await;
                    for field in ["on", "lor"] {
                        if let Some(value) = update.get(field) {
                            state["state"][field] = value.clone();
                        }
                    }
                    if update.get("live") == Some(&serde_json::Value::Bool(false)) {
                        state["info"]["live"] = false.into();
                    }
                    Json(serde_json::json!({"success":true}))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let wled_address = listener.local_addr().unwrap();
    let fixture = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let available = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = available.local_addr().unwrap();
    drop(available);
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("dev.toml");
    std::fs::write(
        &config,
        format!(
            r#"
revision = "0"
source = "prysm"
[processing]
brightness_percent = 100
shadow_lift_percent = 0
smoothing_seconds = 0.0
sample_density = 128
spatial_reach_percent = 10
remove_black_bars = false
[web]
listen = "{address}"
allowed_hosts = ["{address}"]
[wled]
url = "http://{wled_address}/"
[input]
capture_resolution = [4, 4]
refresh_rate_hz = 30
type = "file"
path = "/missing/prysm-local-controller-test.mp4"
[output]
type = "wled"
address = "{output_address}"
[output.leds]
top = 1
right = 1
bottom = 1
left = 1
"#
        ),
    )
    .unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_prysm"))
        .args(["--config"])
        .arg(&config)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let endpoint = format!("http://{address}/api/state");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(response) = client.get(&endpoint).send().await
                && response.status().is_success()
            {
                break;
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "controller exited before serving its API"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    for path in ["/", "/missing", "/api/missing"] {
        let response = client
            .get(format!("http://{address}{path}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
        assert_eq!(
            response.headers()["content-type"],
            "text/plain; charset=utf-8"
        );
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        let message = response.text().await.unwrap();
        assert!(message.contains("Prysm REST API"));
        assert!(message.contains("/api/state"));
        assert!(message.contains("/api/config"));
        assert!(message.contains("8788"));
    }
    let initial = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = wait(&client, &endpoint).await;
            if state.observed.is_some() {
                break state;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        initial.observed.unwrap().pipeline_phase,
        prysm_contract::PipelinePhase::Idle
    );
    let config_endpoint = format!("http://{address}/api/config");
    let mut settings: Config = client
        .get(&config_endpoint)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let revision = settings.revision;
    settings.processing.brightness_percent = 42.try_into().unwrap();
    let saved: Config = client
        .put(&config_endpoint)
        .json(&UpdateConfig {
            expected_revision: revision,
            config: settings,
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(saved.processing.brightness_percent.get(), 42);
    assert_ne!(saved.revision, revision);
    assert_eq!(prysm::config::load(&config).unwrap(), saved);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = wait(&client, &endpoint).await;
            if matches!(state.operation, OperationStatus::Succeeded { .. }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    if !cfg!(feature = "desktop") {
        let before = std::fs::read(&config).unwrap();
        let mut egui = saved.clone();
        egui.output = prysm_contract::Output::Egui {};
        let response = client
            .put(&config_endpoint)
            .json(&UpdateConfig {
                expected_revision: egui.revision,
                config: egui,
            })
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            response
                .text()
                .await
                .unwrap()
                .contains("--features desktop")
        );
        assert_eq!(std::fs::read(&config).unwrap(), before);
    }
    let mut web_settings = saved;
    web_settings.web.listen = "127.0.0.1:0".parse().unwrap();
    web_settings.web.allowed_hosts = vec!["new-panel.local:8787".into()];
    let saved = save_config(&client, &config_endpoint, &endpoint, web_settings).await;
    assert!(wait(&client, &endpoint).await.restart_required);
    assert_eq!(
        client
            .get(&endpoint)
            .header("Host", "new-panel.local:8787")
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::FORBIDDEN,
    );
    // Changing the saved source while Off must leave capture and lights off.
    let mut settings = saved;
    settings.source = Source::Wled;
    save_config(&client, &config_endpoint, &endpoint, settings).await;
    let still_off = wait(&client, &endpoint).await;
    assert!(still_off.observed.unwrap().matches(Target {
        power: false,
        source: Source::Wled,
    }));
    let accepted: OperationResponse = client
        .put(&endpoint)
        .json(&Target {
            power: true,
            source: Source::Wled,
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let effects = wait_operation(&client, &endpoint, accepted).await;
    assert!(matches!(
        effects.operation,
        OperationStatus::Succeeded { .. }
    ));
    assert_eq!(effects.source, Source::Wled);
    // A missing input fails inside the task; the panel stays available and restores effects.
    let accepted = client
        .put(&endpoint)
        .json(&Target {
            power: true,
            source: Source::Prysm,
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let failed = wait_operation(&client, &endpoint, accepted).await;
    assert!(matches!(failed.operation, OperationStatus::Failed { .. }));
    assert!(failed.observed.unwrap().matches(Target {
        power: true,
        source: Source::Wled
    }));
    if let Some(video) = video {
        let mut settings: Config = client
            .get(&config_endpoint)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        settings.input = prysm_contract::Input::File {
            capture_resolution: [640, 360].try_into().unwrap(),
            refresh_rate_hz: 30.try_into().unwrap(),
            path: video.to_str().unwrap().to_owned().try_into().unwrap(),
        };
        if let prysm_contract::Input::File {
            capture_resolution, ..
        } = &mut settings.input
        {
            *capture_resolution = [16, 16].try_into().unwrap();
        }
        // A source saved while effects are on must start screen sync too.
        settings.source = Source::Prysm;
        save_config(&client, &config_endpoint, &endpoint, settings).await;
        let running = wait(&client, &endpoint).await;
        assert!(
            matches!(running.operation, OperationStatus::Succeeded { .. }),
            "{running:?}"
        );
        assert!(running.pipeline.frames_processed > 0);
        assert!(running.observed.unwrap().matches(Target {
            power: true,
            source: Source::Prysm,
        }));

        let mut settings: Config = client
            .get(&config_endpoint)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        settings.processing.brightness_percent = 0.try_into().unwrap();
        let saved = save_config(&client, &config_endpoint, &endpoint, settings).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = wait(&client, &endpoint).await;
                let black = wled.lock().await["pixels"]
                    .as_array()
                    .is_some_and(|pixels| {
                        !pixels.is_empty() && pixels.iter().all(|pixel| pixel == 0)
                    });
                if state.applied_revision == Some(saved.revision)
                    && state.pipeline.frames_processed > running.pipeline.frames_processed
                    && black
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();

        let mut settings = saved;
        if let prysm_contract::Input::File {
            refresh_rate_hz, ..
        } = &mut settings.input
        {
            *refresh_rate_hz = 15.try_into().unwrap();
        }
        let saved = save_config(&client, &config_endpoint, &endpoint, settings).await;
        let restarted = wait(&client, &endpoint).await;
        assert_eq!(restarted.applied_revision, Some(saved.revision));
        assert_eq!(
            restarted.pipeline.phase,
            prysm_contract::PipelinePhase::Running
        );

        let accepted = client
            .put(&endpoint)
            .json(&Target {
                power: true,
                source: Source::Wled,
            })
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        let effects = wait_operation(&client, &endpoint, accepted).await;
        assert!(matches!(
            effects.operation,
            OperationStatus::Succeeded { .. }
        ));
        assert_eq!(effects.pipeline.phase, prysm_contract::PipelinePhase::Idle);
        tokio::time::sleep(Duration::from_millis(50)).await;
        let stopped_count = packets.load(Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(packets.load(Ordering::Relaxed), stopped_count);
    }
    let accepted = client
        .put(&endpoint)
        .json(&Target {
            power: false,
            source: Source::Wled,
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let off = wait_operation(&client, &endpoint, accepted).await;
    assert!(matches!(off.operation, OperationStatus::Succeeded { .. }));
    assert!(!off.observed.unwrap().power);
    // Hold an accepted operation so conflicts and shutdown draining are deterministic.
    pause_next.store(true, Ordering::Relaxed);
    let accepted = client
        .put(&endpoint)
        .json(&Target {
            power: true,
            source: Source::Wled,
        })
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status(), reqwest::StatusCode::ACCEPTED);
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    assert!(matches!(
        wait(&client, &endpoint).await.operation,
        OperationStatus::Pending { .. }
    ));
    assert_eq!(
        client
            .put(&endpoint)
            .json(&Target {
                power: false,
                source: Source::Wled
            })
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::CONFLICT,
    );
    let settings = prysm::config::load(&config).unwrap();
    assert_eq!(
        client
            .put(&config_endpoint)
            .json(&UpdateConfig {
                expected_revision: settings.revision,
                config: settings,
            })
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::CONFLICT,
    );
    assert!(
        tokio::process::Command::new("kill")
            .arg("-TERM")
            .arg(child.id().unwrap().to_string())
            .status()
            .await
            .unwrap()
            .success()
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        child.try_wait().unwrap().is_none(),
        "shutdown must drain accepted work"
    );
    release.notify_one();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert_eq!(wled.lock().await["state"]["on"], true);
    fixture.abort();
    ddp.abort();
}
async fn wait(client: &reqwest::Client, endpoint: &str) -> StateResponse {
    client
        .get(endpoint)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}
async fn wait_operation(
    client: &reqwest::Client,
    endpoint: &str,
    operation: OperationResponse,
) -> StateResponse {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let state = wait(client, endpoint).await;
            if matches!(state.operation,
                OperationStatus::Succeeded { id } | OperationStatus::Failed { id, .. }
                if id == operation.operation_id)
            {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap()
}

async fn save_config(
    client: &reqwest::Client,
    config_endpoint: &str,
    endpoint: &str,
    config: Config,
) -> Config {
    let saved: Config = client
        .put(config_endpoint)
        .json(&UpdateConfig {
            expected_revision: config.revision,
            config,
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let state = wait(client, endpoint).await;
            if state.config_revision == saved.revision
                && !matches!(state.operation, OperationStatus::Pending { .. })
            {
                assert!(
                    matches!(state.operation, OperationStatus::Succeeded { .. }),
                    "{state:?}"
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    saved
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "keep each end-to-end controller scenario and its assertions together"
)]
async fn start_retries_saved_configuration_before_opening_the_input() {
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let desired = occupied.local_addr().unwrap();
    let available = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = available.local_addr().unwrap();
    drop(available);
    let mut config: Config = toml::from_str(include_str!("../../../prysm.toml")).unwrap();
    config.web.listen = address;
    config.web.allowed_hosts = vec!["*".into()];
    config.wled = None;
    config.input = prysm_contract::Input::File {
        path: "/missing/old-input.mkv".to_owned().try_into().unwrap(),
        capture_resolution: [4, 2].try_into().unwrap(),
        refresh_rate_hz: 30.try_into().unwrap(),
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("prysm.toml");
    std::fs::write(&path, toml::to_string(&config).unwrap()).unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_prysm"))
        .arg("--config")
        .arg(&path)
        .kill_on_drop(true)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let endpoint = format!("http://{address}/api/state");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if client.get(&endpoint).send().await.is_ok() {
                break;
            }
            assert!(child.try_wait().unwrap().is_none());
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    config.input = prysm_contract::Input::Webos {
        listen: desired,
        capture_resolution: [4, 2].try_into().unwrap(),
        refresh_rate_hz: 30.try_into().unwrap(),
    };
    let saved: Config = client
        .put(format!("http://{address}/api/config"))
        .json(&UpdateConfig {
            expected_revision: config.revision,
            config,
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = wait(&client, &endpoint).await;
            if state.config_revision == saved.revision
                && matches!(state.operation, OperationStatus::Failed { .. })
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    // Retrying while the desired input is unavailable must never start the old input.
    let accepted = client
        .put(&endpoint)
        .json(&Target {
            power: true,
            source: Source::Prysm,
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let failed = wait_operation(&client, &endpoint, accepted).await;
    let OperationStatus::Failed { error, .. } = failed.operation else {
        panic!("expected bind failure")
    };
    assert!(
        error.message.contains("cannot listen for webOS"),
        "{}",
        error.message
    );
    assert_eq!(failed.pipeline.phase, prysm_contract::PipelinePhase::Idle);
    drop(occupied);
    let accepted = client
        .put(&endpoint)
        .json(&Target {
            power: true,
            source: Source::Prysm,
        })
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let started = wait_operation(&client, &endpoint, accepted).await;
    assert!(
        matches!(started.operation, OperationStatus::Succeeded { .. }),
        "{started:?}"
    );
    assert_eq!(started.applied_revision, Some(saved.revision));
    assert!(tokio::net::TcpStream::connect(desired).await.is_ok());
    child.kill().await.unwrap();
}
