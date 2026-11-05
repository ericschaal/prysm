//! The production controller owns persistence, source changes and shutdown.
use super::*;
use crate::{preview::Preview, settings::SettingsStore, test_support};
use prysm_api::{App, CommandError};
use prysm_contract::{OperationStatus, Source, Target, UpdateSettings};
use std::{sync::Arc, time::Duration};

async fn settled(app: &App) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while matches!(
            app.state().unwrap().operation,
            OperationStatus::Pending { .. }
        ) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn disconnected_settings_caller_still_saves_and_rejects_stale_revision() {
    let directory = tempfile::tempdir().unwrap();
    let config = test_support::config();
    let store = test_support::store(&directory, &config);
    let (controller, app) = Controller::start(
        store,
        Preview::new().publisher(),
        prysm_capture::WebosReceiver::new().unwrap(),
    );
    let initial = app.settings();
    let mut changed = initial.settings.clone();
    changed.processing.brightness_percent = 42.try_into().unwrap();
    let update: UpdateSettings = serde_json::from_value(serde_json::json!({
        "expected_revision":initial.revision,"settings":changed
    }))
    .unwrap();
    {
        let mut request = Box::pin(app.update_settings(update.clone()));
        assert!(futures::poll!(&mut request).is_pending());
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while app.settings().revision == initial.revision {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    settled(&app).await;
    assert_eq!(
        app.settings().settings.processing.brightness_percent.get(),
        42
    );
    assert!(matches!(
        app.state().unwrap().operation,
        OperationStatus::Succeeded { .. }
    ));
    assert_eq!(app.state().unwrap().applied_revision, None);
    let settled_operation = serde_json::to_value(app.state().unwrap().operation).unwrap();
    assert!(matches!(
        app.update_settings(update).await,
        Err(CommandError::Conflict(_))
    ));
    assert_eq!(
        serde_json::to_value(app.state().unwrap().operation).unwrap(),
        settled_operation
    );
    let loaded = SettingsStore::load(directory.path().join("settings.json")).unwrap();
    assert_eq!(loaded.view().settings, app.settings().settings);
    controller.shutdown().await.unwrap();
}

#[tokio::test]
async fn pending_wled_operation_conflicts_and_shutdown_waits_for_accepted_work() {
    target_transaction(false).await;
}

#[tokio::test]
async fn failed_switch_keeps_the_previous_persisted_source() {
    target_transaction(true).await;
}

async fn target_transaction(reject_switch: bool) {
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
    let router = Router::new().route("/json/si",get(move || {
        let on = observed.load(std::sync::atomic::Ordering::Relaxed);
        async move {Json(serde_json::json!({"state":{"on":on,"lor":2},"info":{"live":false,"lm":null}}))}
    })).route("/json/state",post(move |Json(value): Json<serde_json::Value>| {
        let (entered, release, changed) = (command_entered.clone(),command_release.clone(),changed.clone());
        async move {
            if value["on"] == true {entered.notify_one(); release.notified().await; changed.store(true,std::sync::atomic::Ordering::Relaxed);}
            (if reject_switch && value["on"] == true { axum::http::StatusCode::BAD_GATEWAY } else { axum::http::StatusCode::OK },Json(serde_json::json!({"success":!reject_switch})))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let fixture_shutdown = tokio_util::sync::CancellationToken::new();
    let stopped = fixture_shutdown.clone();
    let fixture = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(stopped.cancelled_owned())
            .await;
    });
    let directory = tempfile::tempdir().unwrap();
    let mut config = test_support::config();
    config.source = Source::Prysm;
    let prysm_contract::Output::Wled { url, .. } = &mut config.output;
    *url = format!("http://{address}/").parse().unwrap();

    let store = test_support::store(&directory, &config);
    let (controller, app) = Controller::start(
        store,
        Preview::new().publisher(),
        prysm_capture::WebosReceiver::new().unwrap(),
    );
    let accepted = app
        .set_target(Target {
            power: true,
            source: Source::Wled,
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    assert!(matches!(
        app.update_settings(test_support::update(&app, &config))
            .await,
        Err(CommandError::Conflict(_))
    ));
    assert!(
        matches!(app.state().unwrap().operation,OperationStatus::Pending {id} if id==accepted.operation_id)
    );
    let stopping = controller.shutdown();
    tokio::pin!(stopping);
    assert!(
        futures::poll!(&mut stopping).is_pending(),
        "shutdown retains accepted work"
    );
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), stopping)
        .await
        .unwrap()
        .unwrap();
    let state = app.state().unwrap();
    if reject_switch {
        assert!(matches!(state.operation, OperationStatus::Failed { .. }));
    } else {
        assert!(matches!(state.operation, OperationStatus::Succeeded { .. }));
    }
    let loaded = SettingsStore::load(directory.path().join("settings.json")).unwrap();
    assert_eq!(
        loaded.view().settings.source,
        if reject_switch {
            Source::Prysm
        } else {
            Source::Wled
        }
    );
    assert_eq!(app.settings().settings, loaded.view().settings);
    assert_eq!(state.config_revision, app.settings().revision);
    assert!(on.load(std::sync::atomic::Ordering::Relaxed));
    fixture_shutdown.cancel();
    fixture.await.unwrap();
}

#[tokio::test]
async fn persistence_failure_preserves_settings_and_revision() {
    let directory = tempfile::tempdir().unwrap();
    let config = test_support::config();
    let store = test_support::store(&directory, &config);
    let path = directory.path().join("settings.json");
    let (controller, app) = Controller::start(
        store,
        Preview::new().publisher(),
        prysm_capture::WebosReceiver::new().unwrap(),
    );
    let initial = app.settings();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let mut changed = config;
    changed.processing.brightness_percent = 42.try_into().unwrap();
    assert!(matches!(
        app.update_settings(test_support::update(&app, &changed))
            .await,
        Err(CommandError::Persistence(_))
    ));
    assert_eq!(app.settings().revision, initial.revision);
    assert_eq!(app.settings().settings, initial.settings);
    assert!(matches!(
        app.state().unwrap().operation,
        OperationStatus::Idle
    ));
    assert!(path.is_dir());
    controller.shutdown().await.unwrap();
}
