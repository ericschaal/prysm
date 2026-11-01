//! Run explicitly on a machine with a desktop session and `FFmpeg` 9+.
#![cfg(all(feature = "desktop", unix))]

use prysm_contract::{Config, Input, PipelinePhase, Source, StateResponse, Target};
use std::{process::Stdio, time::Duration};

#[tokio::test]
#[ignore = "requires FFmpeg 9+ and a native desktop session"]
#[allow(
    clippy::too_many_lines,
    reason = "keep the full desktop startup and shutdown scenario together"
)]
async fn file_egui_keeps_final_frame_and_shuts_down_on_signal() {
    let directory = tempfile::tempdir().unwrap();
    let video = directory.path().join("egui.mkv");
    assert!(
        tokio::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=16x16:r=30",
                "-t",
                "0.3",
                "-c:v",
                "ffv1"
            ])
            .arg(&video)
            .status()
            .await
            .unwrap()
            .success()
    );
    let mut config: Config = toml::from_str(include_str!("../../../prysm-egui.toml")).unwrap();
    assert!(config.wled.is_none());
    config.input = Input::File {
        capture_resolution: [640, 360].try_into().unwrap(),
        refresh_rate_hz: 30.try_into().unwrap(),
        path: video.to_str().unwrap().to_owned().try_into().unwrap(),
    };
    if let prysm_contract::Input::File {
        capture_resolution, ..
    } = &mut config.input
    {
        *capture_resolution = [16, 16].try_into().unwrap();
    }
    let available = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = available.local_addr().unwrap();
    drop(available);
    config.web.listen = address;
    config.web.allowed_hosts = vec![address.to_string()];
    let path = directory.path().join("egui.toml");
    std::fs::write(&path, toml::to_string(&config).unwrap()).unwrap();
    let logs = directory.path().join("egui.log");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_prysm"))
        .arg("--config")
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&logs).unwrap())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let endpoint = format!("http://{address}/api/state");
    let initial: StateResponse = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Ok(response) = client.get(&endpoint).send().await
                && response.status().is_success()
            {
                break response.json().await.unwrap();
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "{}",
                std::fs::read_to_string(&logs).unwrap()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let page = client
        .get(format!("http://{address}/"))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), reqwest::StatusCode::NOT_FOUND);
    assert_eq!(initial.pipeline.phase, PipelinePhase::Idle);
    client
        .put(&endpoint)
        .json(&Target {
            power: true,
            source: Source::Prysm,
        })
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let output = std::fs::read_to_string(&logs).unwrap();
            assert!(
                child.try_wait().unwrap().is_none(),
                "egui exited early: {output}"
            );
            if output.contains("Video playback complete") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let completed: StateResponse = client
        .get(&endpoint)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    // Polling can lag the pipeline by one controller observation interval.
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut state = completed;
        while state.pipeline.phase != PipelinePhase::Completed {
            tokio::time::sleep(Duration::from_millis(20)).await;
            state = client
                .get(&endpoint)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
        }
        assert!(state.observed.unwrap().power);
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        child.try_wait().unwrap().is_none(),
        "egui must keep the final frame visible"
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
    let result = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(
        result.success(),
        "{}",
        std::fs::read_to_string(&logs).unwrap()
    );
}
