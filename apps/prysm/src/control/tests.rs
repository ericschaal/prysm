//! Real command/reply and persistence behavior, with WLED isolated behind a loopback fixture.
use super::*;
use crate::{config::PipelineConfig, pipeline::runtime::Pipeline};
use prysm_api::{App, Command};
use prysm_contract::{Config, OperationStatus, Source, Target, UpdateConfig};
use std::{sync::Arc, time::Duration};
use tokio::sync::{oneshot, watch};

async fn settled(app: &mut App) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !matches!(
                app.state.borrow_and_update().operation,
                OperationStatus::Pending { .. }
            ) {
                break;
            }
            app.state.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn disconnected_config_caller_still_saves_applies_and_rejects_stale_revision() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("prysm.toml");
    let mut config: Config = toml::from_str(include_str!("../../../../prysm.toml")).unwrap();
    // No real hardware/network endpoint is contacted by configuration application.
    let prysm_contract::Output::Wled { url, .. } = &mut config.output;
    {
        *url = "http://127.0.0.1:1/".parse().unwrap();
    }
    let (config_tx, config_rx) = watch::channel(config.clone());
    let (pipeline, status) = Pipeline::spawn(PipelineConfig::from(&config)).unwrap();
    let lighting = Arc::new(LightingService::new(config_rx, pipeline.clone(), status).unwrap());
    let (controller, mut app) = Controller::spawn(path.clone(), &config, lighting, config_tx);
    config.processing.brightness_percent = 42.try_into().unwrap();
    let (reply, response) = oneshot::channel();
    drop(response);
    app.commands
        .send(Command::SetConfig(
            Box::new(UpdateConfig {
                expected_revision: config.revision,
                config: config.clone(),
            }),
            reply,
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while app.config.borrow_and_update().revision == config.revision {
            app.config.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    settled(&mut app).await;
    assert!(matches!(
        app.state.borrow().operation,
        OperationStatus::Succeeded { .. }
    ));
    let saved = crate::config::load(&path).unwrap();
    assert_eq!(saved.processing.brightness_percent.get(), 42);
    assert_eq!(*app.config.borrow(), saved);
    assert_eq!(
        app.state.borrow().applied_revision,
        None,
        "idle configuration is applied when processing starts"
    );
    let (reply, response) = oneshot::channel();
    app.commands
        .send(Command::SetConfig(
            Box::new(UpdateConfig {
                expected_revision: config.revision,
                config,
            }),
            reply,
        ))
        .await
        .unwrap();
    assert!(response.await.unwrap().is_err());
    assert_eq!(crate::config::load(&path).unwrap(), saved);
    controller.shutdown().await.unwrap();
    pipeline.shutdown().await.unwrap();
}

#[tokio::test]
async fn pending_wled_operation_conflicts_and_shutdown_waits_for_accepted_work() {
    target_transaction(false).await;
}

#[tokio::test]
async fn source_saved_before_apply_failure_keeps_authoritative_revision() {
    target_transaction(true).await;
}

#[allow(
    clippy::too_many_lines,
    reason = "keep the gated WLED transaction and shutdown assertions together"
)]
async fn target_transaction(retire_pipeline: bool) {
    use axum::{
        Json, Router,
        routing::{get, post},
    };
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let on = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = on.clone();
    let changed = on.clone();
    let command_entered = entered.clone();
    let command_release = release.clone();
    let router = Router::new().route("/json/si", get(move || {
        let on = observed.load(std::sync::atomic::Ordering::Relaxed);
        async move { Json(serde_json::json!({"state":{"on":on,"lor":2},"info":{"live":false,"lm":null}})) }
    })).route("/json/state", post(move |Json(value): Json<serde_json::Value>| {
        let (entered, release, changed) = (command_entered.clone(), command_release.clone(), changed.clone());
        async move {
            if value["on"] == true {
                entered.notify_one();
                release.notified().await;
                changed.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            Json(serde_json::json!({"success":true}))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let fixture_shutdown = tokio_util::sync::CancellationToken::new();
    let stopped = fixture_shutdown.clone();
    let fixture = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(stopped.cancelled_owned())
            .await
            .unwrap();
    });
    let directory = tempfile::tempdir().unwrap();
    let mut config: Config = toml::from_str(include_str!("../../../../prysm.toml")).unwrap();
    config.source = Source::Prysm;
    let prysm_contract::Output::Wled { url, .. } = &mut config.output;
    {
        *url = format!("http://{address}/").parse().unwrap();
    }
    let (config_tx, config_rx) = watch::channel(config.clone());
    let (pipeline, status) = Pipeline::spawn(PipelineConfig::from(&config)).unwrap();
    let lighting = Arc::new(LightingService::new(config_rx, pipeline.clone(), status).unwrap());
    let (controller, mut app) = Controller::spawn(
        directory.path().join("prysm.toml"),
        &config,
        lighting,
        config_tx,
    );
    let (reply, response) = oneshot::channel();
    app.commands
        .send(Command::SetTarget(
            Target {
                power: true,
                source: Source::Wled,
            },
            reply,
        ))
        .await
        .unwrap();
    let accepted = response.await.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    let (reply, response) = oneshot::channel();
    app.commands
        .send(Command::SetConfig(
            Box::new(UpdateConfig {
                expected_revision: config.revision,
                config,
            }),
            reply,
        ))
        .await
        .unwrap();
    assert!(response.await.unwrap().is_err());
    assert!(
        matches!(app.state.borrow().operation, OperationStatus::Pending { id } if id == accepted.operation_id)
    );
    if retire_pipeline {
        pipeline.shutdown().await.unwrap();
    }
    let stopping = controller.shutdown();
    tokio::pin!(stopping);
    assert!(
        futures::poll!(&mut stopping).is_pending(),
        "shutdown must retain accepted work"
    );
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), stopping)
        .await
        .unwrap()
        .unwrap();
    settled(&mut app).await;
    if retire_pipeline {
        assert!(matches!(
            app.state.borrow().operation,
            OperationStatus::Failed { .. }
        ));
    } else {
        assert!(matches!(
            app.state.borrow().operation,
            OperationStatus::Succeeded { .. }
        ));
    }
    let saved = crate::config::load(&directory.path().join("prysm.toml")).unwrap();
    assert_eq!(saved.source, Source::Wled);
    assert_eq!(*app.config.borrow(), saved);
    assert_eq!(app.state.borrow().config_revision, saved.revision);
    assert!(on.load(std::sync::atomic::Ordering::Relaxed));
    if !retire_pipeline {
        pipeline.shutdown().await.unwrap();
    }
    fixture_shutdown.cancel();
    fixture.await.unwrap();
}

#[tokio::test]
async fn persistence_failure_preserves_saved_config_and_revision() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config-is-a-directory");
    std::fs::create_dir(&path).unwrap();
    let mut config: Config = toml::from_str(include_str!("../../../../prysm.toml")).unwrap();
    let prysm_contract::Output::Wled { url, .. } = &mut config.output;
    *url = "http://127.0.0.1:1/".parse().unwrap();
    let (config_tx, config_rx) = watch::channel(config.clone());
    let (pipeline, status) = Pipeline::spawn(PipelineConfig::from(&config)).unwrap();
    let lighting = Arc::new(LightingService::new(config_rx, pipeline.clone(), status).unwrap());
    let (controller, app) = Controller::spawn(path.clone(), &config, lighting, config_tx);
    let mut changed = config.clone();
    changed.processing.brightness_percent = 42.try_into().unwrap();
    let (reply, response) = oneshot::channel();
    app.commands
        .send(Command::SetConfig(
            Box::new(UpdateConfig {
                expected_revision: config.revision,
                config: changed,
            }),
            reply,
        ))
        .await
        .unwrap();
    assert!(response.await.unwrap().is_err());
    assert_eq!(*app.config.borrow(), config);
    assert_eq!(app.state.borrow().config_revision, config.revision);
    assert!(matches!(
        app.state.borrow().operation,
        OperationStatus::Failed { .. }
    ));
    assert!(path.is_dir());
    controller.shutdown().await.unwrap();
    pipeline.shutdown().await.unwrap();
}
