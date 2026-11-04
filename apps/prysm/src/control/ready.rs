//! A configured lighting session owns its pipeline and WLED observer together.
use super::backend::{CaptureAction, LightingBackend, LightingService, LightingSnapshot};
use crate::{config::PipelineConfig, pipeline::runtime::Pipeline, preview::Publisher};
use anyhow::{Context as _, Result};
use prysm_api::CommandError;
use prysm_contract::{
    Config, ConfigRevision, Observation, OperationStatus, PipelinePhase, PipelineStatus, Source,
    StateResponse, Target,
};
use std::{future::Future, sync::Arc};
use tokio::{sync::watch, task::JoinHandle};
use tokio_util::sync::CancellationToken;

type Observations = watch::Receiver<Option<(ConfigRevision, LightingSnapshot)>>;

pub(super) struct OperationResult {
    pub result: Result<(), CommandError>,
    pub observation: CompletionSnapshot,
}

pub(super) struct CompletionSnapshot {
    snapshot: LightingSnapshot,
    observations: Observations,
}

impl OperationResult {
    async fn capture(
        service: &LightingService,
        result: Result<(), CommandError>,
        mut observations: Observations,
    ) -> Self {
        observations.borrow_and_update();
        let snapshot = service.snapshot().await;
        Self {
            result,
            observation: CompletionSnapshot {
                snapshot,
                observations,
            },
        }
    }
}

pub(super) struct Ready {
    config: Config,
    service: Arc<LightingService>,
    config_tx: watch::Sender<Config>,
    updates: Observations,
    observer: JoinHandle<()>,
    shutdown: CancellationToken,
    observed: Option<Observation>,
    observation_error: Option<String>,
}

impl Ready {
    pub fn new(
        config: &Config,
        preview: Publisher,
        webos: prysm_capture::WebosReceiver,
    ) -> Result<Self> {
        let client = LightingService::client()?;
        let (config_tx, config_rx) = watch::channel(config.clone());
        let (pipeline, status) =
            Pipeline::with_preview(PipelineConfig::from(config), webos, preview);
        let service = Arc::new(LightingService::new(client, config_rx, pipeline, status));
        let shutdown = CancellationToken::new();
        let stopped = shutdown.clone();
        let watched = service.clone();
        let (updates, receiver) = watch::channel(None);
        let observer = tokio::spawn(async move {
            tokio::select! {
                () = stopped.cancelled() => {},
                () = watched.observations(updates) => {},
            }
        });
        Ok(Self {
            config: config.clone(),
            service,
            config_tx,
            updates: receiver,
            observer,
            shutdown,
            observed: None,
            observation_error: None,
        })
    }

    pub fn state(&self, operation: OperationStatus) -> StateResponse {
        let pipeline = self.service.status.borrow().clone();
        let mut observed = self.observed.clone();
        if let Some(observed) = &mut observed {
            observed.pipeline_phase = pipeline.phase;
        }
        StateResponse {
            calibration_active: false,
            source: self.config.source,
            observed,
            observation_error: self.observation_error.clone(),
            operation,
            wled_url: self
                .config
                .output
                .wled_url()
                .map(ToString::to_string)
                .unwrap_or_default(),
            config_revision: self.config.revision,
            applied_revision: pipeline.applied_revision,
            pipeline,
        }
    }

    pub fn record_settings(&mut self, config: Config) {
        if config.output != self.config.output {
            self.observed = None;
            self.observation_error = None;
        }
        self.config_tx.send_replace(config.clone());
        self.config = config;
    }

    pub async fn live(&mut self, config: Config) -> Result<(), CommandError> {
        self.service
            .pipeline
            .configure(PipelineConfig::from(&config))
            .await
            .map_err(CommandError::Backend)?;
        self.record_settings(config);
        Ok(())
    }

    pub fn apply(
        &mut self,
        config: Config,
    ) -> impl Future<Output = OperationResult> + Send + 'static {
        let previous = self.config.clone();
        self.record_settings(config.clone());
        let service = self.service.clone();
        let observations = self.updates.clone();
        async move {
            let result = async {
                let running = matches!(
                    service.status.borrow().phase,
                    PipelinePhase::Starting | PipelinePhase::Running
                );
                let source_changed = previous.source != config.source;
                let reconnect = PipelineConfig::from(&previous)
                    .requires_restart(&PipelineConfig::from(&config));
                if running && (reconnect || source_changed) {
                    service.capture(CaptureAction::Stop).await?;
                }
                service
                    .pipeline
                    .configure(PipelineConfig::from(&config))
                    .await?;
                if (running && reconnect) || source_changed {
                    let power = service.observe().await?.power;
                    super::switching::switch(
                        &*service,
                        Target {
                            power,
                            source: config.source,
                        },
                    )
                    .await?;
                }
                Ok(())
            }
            .await
            .map_err(CommandError::Backend);
            OperationResult::capture(&service, result, observations).await
        }
    }

    pub fn switch(&self, target: Target) -> impl Future<Output = OperationResult> + Send + 'static {
        let service = self.service.clone();
        let configured = PipelineConfig::from(&self.config);
        let observations = self.updates.clone();
        async move {
            let result = async {
                if target.power && target.source == Source::Prysm {
                    service
                        .pipeline
                        .configure(configured)
                        .await
                        .map_err(CommandError::Backend)?;
                }
                super::switching::switch(&*service, target)
                    .await
                    .map_err(CommandError::Backend)?;
                Ok(())
            }
            .await;
            OperationResult::capture(&service, result, observations).await
        }
    }

    pub fn start_calibration(
        &self,
        cancel: CancellationToken,
    ) -> impl Future<Output = OperationResult> + Send + 'static {
        let service = self.service.clone();
        let observations = self.updates.clone();
        async move {
            let result = super::switching::apply(
                &*service,
                Target {
                    power: true,
                    source: Source::Prysm,
                },
                &cancel,
            )
            .await
            .map_err(CommandError::Backend);
            OperationResult::capture(&service, result, observations).await
        }
    }

    pub async fn source_saved(&mut self, config: Config) -> Result<(), CommandError> {
        self.record_settings(config);
        self.service
            .pipeline
            .configure(PipelineConfig::from(&self.config))
            .await
            .map_err(CommandError::Backend)
    }

    pub async fn observe(&mut self) -> Result<()> {
        self.updates
            .changed()
            .await
            .context("WLED observer stopped unexpectedly")?;
        let update = self.updates.borrow_and_update().clone();
        if let Some((revision, snapshot)) = update
            && revision == self.config.revision
        {
            self.record_snapshot(snapshot);
        }
        Ok(())
    }

    pub fn finish(&mut self, operation: CompletionSnapshot) {
        if !operation.observations.has_changed().unwrap_or(true) {
            self.record_snapshot(operation.snapshot);
        }
    }

    fn record_snapshot(&mut self, snapshot: LightingSnapshot) {
        self.observed = snapshot.observed;
        self.observation_error = snapshot.error;
    }

    pub async fn shutdown(self) -> Result<()> {
        self.shutdown.cancel();
        let observer = self.observer.await.context("WLED observer panicked");
        let pipeline = self.service.pipeline.shutdown().await;
        observer?;
        pipeline
    }
}

pub(super) fn failed_state(config: &Config, error: &anyhow::Error) -> StateResponse {
    StateResponse {
        calibration_active: false,
        source: config.source,
        observed: None,
        observation_error: Some(format!("{error:#}")),
        operation: OperationStatus::Idle,
        wled_url: config
            .output
            .wled_url()
            .map(ToString::to_string)
            .unwrap_or_default(),
        config_revision: config.revision,
        applied_revision: None,
        pipeline: PipelineStatus {
            phase: PipelinePhase::Failed,
            applied_revision: None,
            frames_processed: 0,
            error: Some(format!("{error:#}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{preview::Preview, test_support};

    #[tokio::test]
    async fn changing_source_stops_capture_even_when_wled_is_unreachable() {
        let mut config = test_support::config();
        config.source = Source::Prysm;
        config.input = prysm_contract::Input::Webos {
            capture_resolution: [4, 2].try_into().unwrap(),
            refresh_rate_hz: 30.try_into().unwrap(),
        };
        let mut ready = Ready::new(
            &config,
            Preview::new().publisher(),
            prysm_capture::WebosReceiver::new().unwrap(),
        )
        .unwrap();
        ready.service.pipeline.start().await.unwrap();
        config.source = Source::Wled;
        config.revision = config.revision.next().unwrap();
        let operation = ready.apply(config).await;
        assert!(operation.result.is_err());
        assert_eq!(ready.service.status.borrow().phase, PipelinePhase::Idle);
        ready.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn changing_output_during_capture_does_not_power_on_the_new_lights() {
        use axum::{
            Json, Router,
            routing::{get, post},
        };
        use std::sync::atomic::{AtomicBool, Ordering};
        let turned_on = Arc::new(AtomicBool::new(false));
        let commands = turned_on.clone();
        let router = Router::new()
            .route(
                "/json/si",
                get(|| async {
                    Json(serde_json::json!({
                        "state":{"on":false,"lor":2}, "info":{"live":false,"lm":null}
                    }))
                }),
            )
            .route(
                "/json/state",
                post(move |Json(value): Json<serde_json::Value>| {
                    let commands = commands.clone();
                    async move {
                        if value["on"] == true {
                            commands.store(true, Ordering::Relaxed);
                        }
                        Json(serde_json::json!({"success":true}))
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let mut config = test_support::config();
        config.source = Source::Prysm;
        config.input = prysm_contract::Input::Webos {
            capture_resolution: [4, 2].try_into().unwrap(),
            refresh_rate_hz: 30.try_into().unwrap(),
        };
        let mut ready = Ready::new(
            &config,
            Preview::new().publisher(),
            prysm_capture::WebosReceiver::new().unwrap(),
        )
        .unwrap();
        // A running receiver needs no physical TV to exercise settings reconciliation.
        ready.service.pipeline.start().await.unwrap();
        assert!(matches!(
            ready.service.status.borrow().phase,
            PipelinePhase::Starting | PipelinePhase::Running
        ));
        let prysm_contract::Output::Wled { url: target, .. } = &mut config.output;
        *target = url.parse().unwrap();
        config.revision = config.revision.next().unwrap();
        let operation = ready.apply(config).await;
        operation.result.unwrap();
        assert!(!turned_on.load(Ordering::Relaxed));
        assert_eq!(ready.service.status.borrow().phase, PipelinePhase::Idle);
        ready.shutdown().await.unwrap();
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn retired_observations_cannot_replace_current_configuration() {
        let mut config = test_support::config();
        let mut ready = Ready::new(
            &config,
            Preview::new().publisher(),
            prysm_capture::WebosReceiver::new().unwrap(),
        )
        .unwrap();
        let (updates, receiver) = watch::channel(None);
        ready.updates = receiver;
        ready.observation_error = Some("current error".into());
        let retired_revision = config.revision;
        config.revision = config.revision.next().unwrap();
        ready.record_settings(config);
        updates.send_replace(Some((
            retired_revision,
            LightingSnapshot {
                observed: None,
                error: Some("retired error".into()),
            },
        )));
        ready.observe().await.unwrap();
        let state = ready.state(OperationStatus::Idle);
        assert_eq!(state.observation_error.as_deref(), Some("current error"));
        assert_eq!(state.pipeline.phase, PipelinePhase::Idle);
        ready.shutdown().await.unwrap();
    }
}
