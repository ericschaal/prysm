//! Exercise the real HTTP router and controller with egui channels, without a window.
#![cfg(feature = "desktop")]

use super::*;
use crate::actors::egui::Egui;
use axum::{body::Body, http::Request};
use prysm_api::App;
use prysm_capture::Frame;
use prysm_contract::{
    Input, OperationStatus, PipelinePhase, Source, StateResponse, Target, UpdateConfig,
};
use prysm_core::{EdgeColors, LinearColor};
use prysm_renderer_desktop::DisplayFrame;
use tower::ServiceExt;

struct EguiController {
    directory: tempfile::TempDir,
    path: std::path::PathBuf,
    router: axum::Router,
    app: App,
    lighting: Arc<LightingService>,
    actor: actix::Addr<Controller>,
    colors: watch::Receiver<EdgeColors>,
    frames: watch::Receiver<DisplayFrame>,
}

impl EguiController {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("egui.toml");
        let mut config: Config =
            toml::from_str(include_str!("../../../../prysm-egui.toml")).unwrap();
        config.input = Input::File {
            refresh_rate_hz: 30.try_into().unwrap(),
            path: "/missing/prysm-egui-test.mkv"
                .to_owned()
                .try_into()
                .unwrap(),
        };
        config.capture_resolution = [16, 16].try_into().unwrap();
        config.processing.smoothing_seconds = 0.0.try_into().unwrap();
        config.processing.remove_black_bars = false;
        std::fs::write(&path, toml::to_string(&config).unwrap()).unwrap();
        let (config_tx, config_rx) = watch::channel(config.clone());
        let (colors, colors_rx) = watch::channel(EdgeColors::default());
        let (frames, frames_rx) = watch::channel(DisplayFrame {
            frame: Frame::dummy(16, 16),
            viewport: None,
        });
        let lighting = Arc::new(
            LightingService::new(
                config_rx,
                Renderers {
                    egui: Some(Egui { colors, frames }),
                },
            )
            .unwrap(),
        );
        let (actor, app) = Controller::spawn(path.clone(), &config, lighting.clone(), config_tx);
        let router = http::router(app.clone(), config.web);
        Self {
            directory,
            path,
            router,
            app,
            lighting,
            actor,
            colors: colors_rx,
            frames: frames_rx,
        }
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        body: impl serde::Serialize,
    ) -> axum::response::Response {
        self.router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("Host", "localhost:8787")
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn settled(&mut self) -> StateResponse {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = self.app.state.borrow_and_update().clone();
                if !matches!(
                    state.operation,
                    OperationStatus::Idle | OperationStatus::Pending { .. }
                ) {
                    return state;
                }
                self.app.state.changed().await.unwrap();
            }
        })
        .await
        .unwrap()
    }

    async fn target(&mut self, power: bool) -> StateResponse {
        let response = self
            .request(
                "PUT",
                "/api/state",
                Target {
                    power,
                    source: Source::Prysm,
                },
            )
            .await;
        assert_eq!(response.status(), axum::http::StatusCode::ACCEPTED);
        // The previous operation may still be the latest published snapshot.
        let accepted: prysm_contract::OperationResponse = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap(),
        )
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = self.settled().await;
                if matches!(state.operation, OperationStatus::Succeeded { id } | OperationStatus::Failed { id, .. }
                    if id == accepted.operation_id) {
                    return state;
                }
                self.app.state.changed().await.unwrap();
            }
        }).await.unwrap()
    }

    async fn save(&mut self, config: Config) -> StateResponse {
        let response = self
            .request(
                "PUT",
                "/api/config",
                UpdateConfig {
                    expected_revision: config.revision,
                    config,
                },
            )
            .await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let saved: Config = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 65536)
                .await
                .unwrap(),
        )
        .unwrap();
        let state = self.settled().await;
        assert_eq!(state.config_revision, saved.revision);
        assert_eq!(crate::config::load(&self.path).unwrap(), saved);
        state
    }

    async fn shutdown(self) {
        self.actor.send(Shutdown).await.unwrap();
        self.lighting
            .pipeline
            .send(StopPipeline)
            .await
            .unwrap()
            .unwrap();
    }
}

#[actix::test]
async fn egui_controller_serves_state_handles_failure_and_saves_settings_without_wled() {
    let mut egui = EguiController::new();
    let response = egui.request("GET", "/api/state", ()).await;
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let snapshot = egui.lighting.snapshot().await;
    assert!(!snapshot.observed.unwrap().power);
    assert!(snapshot.error.is_none());
    assert!(egui.app.config.borrow().wled.is_none());

    let state = egui.target(true).await;
    assert!(matches!(state.operation, OperationStatus::Failed { .. }));
    assert_eq!(state.pipeline.phase, PipelinePhase::Failed);
    assert!(!state.observed.unwrap().power);
    assert!(state.observation_error.is_none());

    let mut config = egui.app.config.borrow().clone();
    config.processing.shadow_lift_percent = 20.try_into().unwrap();
    let saved = egui.save(config).await;
    assert!(matches!(saved.operation, OperationStatus::Succeeded { .. }));
    assert!(!saved.restart_required);
    let response = egui
        .request(
            "PUT",
            "/api/state",
            Target {
                power: true,
                source: Source::Wled,
            },
        )
        .await;
    assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);

    let state = egui.target(false).await;
    assert_eq!(state.pipeline.phase, PipelinePhase::Idle);
    let mut config = egui.app.config.borrow().clone();
    if let Input::File {
        refresh_rate_hz, ..
    } = &mut config.input
    {
        *refresh_rate_hz = 15.try_into().unwrap();
    }
    let saved = egui.save(config).await;
    assert!(!saved.restart_required);
    let response = egui
        .request(
            "PUT",
            "/api/state",
            Target {
                power: true,
                source: Source::Prysm,
            },
        )
        .await;
    assert_eq!(response.status(), axum::http::StatusCode::ACCEPTED);
    egui.shutdown().await;
}

#[actix::test]
#[ignore = "requires FFmpeg 9+ on PATH"]
async fn egui_controller_processes_file_applies_settings_restarts_and_replays_after_eof() {
    let mut egui = EguiController::new();
    let video = egui.directory.path().join("white.mkv");
    assert!(
        std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=white:s=16x16:r=30",
                "-t",
                "2",
                "-c:v",
                "ffv1",
            ])
            .arg(&video)
            .status()
            .unwrap()
            .success()
    );
    let mut config = egui.app.config.borrow().clone();
    config.input = Input::File {
        refresh_rate_hz: 30.try_into().unwrap(),
        path: video.to_str().unwrap().to_owned().try_into().unwrap(),
    };
    assert!(matches!(
        egui.save(config).await.operation,
        OperationStatus::Succeeded { .. }
    ));
    assert_eq!(egui.lighting.status.borrow().phase, PipelinePhase::Idle);

    assert!(matches!(
        egui.target(true).await.operation,
        OperationStatus::Succeeded { .. }
    ));
    tokio::time::timeout(Duration::from_secs(5), async {
        while egui.colors.borrow().top.sample_at(0.5).r < 0.9 {
            egui.colors.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(egui.frames.borrow().frame.width, 16);
    let mut config = egui.app.config.borrow().clone();
    config.processing.brightness_percent = 0.try_into().unwrap();
    assert!(!egui.save(config).await.restart_required);
    tokio::time::timeout(Duration::from_secs(5), async {
        while egui.colors.borrow().top.sample_at(0.5) != LinearColor::black() {
            egui.colors.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(
        egui.lighting.status.borrow().applied_revision,
        Some(egui.app.config.borrow().revision)
    );

    let mut config = egui.app.config.borrow().clone();
    config.capture_resolution = [8, 8].try_into().unwrap();
    assert!(matches!(
        egui.save(config).await.operation,
        OperationStatus::Succeeded { .. }
    ));
    tokio::time::timeout(Duration::from_secs(5), async {
        while egui.frames.borrow().frame.width != 8 {
            egui.frames.changed().await.unwrap();
        }
        let mut status = egui.lighting.status.clone();
        while status.borrow().phase != PipelinePhase::Completed {
            status.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(egui.lighting.snapshot().await.observed.unwrap().power);
    assert!(matches!(
        egui.target(false).await.operation,
        OperationStatus::Succeeded { .. }
    ));
    assert_eq!(egui.lighting.status.borrow().phase, PipelinePhase::Idle);
    assert!(matches!(
        egui.target(true).await.operation,
        OperationStatus::Succeeded { .. }
    ));
    assert_ne!(
        egui.lighting.status.borrow().phase,
        PipelinePhase::Completed
    );
    egui.shutdown().await;
}
