//! One owner serializes accepted mutations while observations and shutdown keep progressing.
use super::{
    backend::{CaptureAction, LightingBackend, LightingService, LightingSnapshot},
    persistence,
};
use crate::config::PipelineConfig;
use anyhow::{Context as _, Result};
use axum::http::StatusCode;
use prysm_api::{ApiFailure, App, Command};
use prysm_contract::{
    ApiError, Config, ConfigRevision, ErrorCode, OperationId, OperationResponse, OperationStatus,
    PipelinePhase, PipelineStatus, StateResponse, Target, WebConfig,
};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

type Observations = watch::Receiver<Option<(ConfigRevision, LightingSnapshot)>>;

fn initial_state(config: &Config) -> StateResponse {
    StateResponse {
        source: config.source,
        observed: None,
        observation_error: None,
        operation: OperationStatus::Idle,
        wled_url: config
            .output
            .wled_url()
            .map(ToString::to_string)
            .unwrap_or_default(),
        config_revision: config.revision,
        applied_revision: None,
        pipeline: PipelineStatus {
            phase: PipelinePhase::Idle,
            applied_revision: None,
            frames_processed: 0,
            error: None,
        },
        restart_required: false,
    }
}
fn update_observation(state: &mut StateResponse, snapshot: LightingSnapshot) {
    state.observed = snapshot.observed;
    state.observation_error = snapshot.error;
    state.applied_revision = snapshot.pipeline.applied_revision;
    state.pipeline = snapshot.pipeline;
}
fn failed_operation(id: OperationId, error: &anyhow::Error) -> OperationStatus {
    tracing::error!(operation_id = ?id, error=%format!("{error:#}"),"operation failed");
    OperationStatus::Failed {
        id,
        error: ApiError {
            code: ErrorCode::Backend,
            message: format!("{error:#}"),
        },
    }
}
pub(crate) struct Controller {
    path: PathBuf,
    config: Config,
    active_web: WebConfig,
    config_tx: watch::Sender<Config>,
    state: StateResponse,
    state_tx: watch::Sender<StateResponse>,
    stopping: bool,
    lighting: Arc<LightingService>,
}

pub(crate) struct ControllerHandle {
    shutdown: CancellationToken,
    task: JoinHandle<Result<()>>,
}
impl ControllerHandle {
    pub async fn shutdown(self) -> Result<()> {
        self.shutdown.cancel();
        self.task.await.context("controller task panicked")?
    }
}
enum Finished {
    Saved {
        id: OperationId,
        previous: Config,
        result: Result<Config>,
        reply: oneshot::Sender<Result<Config, ApiFailure>>,
    },
    Target {
        id: OperationId,
        saved: Option<Config>,
        result: Result<()>,
        snapshot: LightingSnapshot,
        observations: Observations,
    },
    Applied {
        id: OperationId,
        result: Result<()>,
        snapshot: LightingSnapshot,
        observations: Observations,
    },
}

impl Controller {
    async fn save(path: PathBuf, mut config: Config) -> Result<Config> {
        crate::validate_config(&config)?;
        config.revision = config.revision.next().map_err(anyhow::Error::msg)?;
        persistence::save(path, config.clone()).await?;
        Ok(config)
    }
    fn record_saved_config(&mut self, config: Config) {
        if self.config.output != config.output {
            self.state.observed = None;
            self.state.observation_error = None;
        }
        self.config = config;
        self.config_tx.send_replace(self.config.clone());
        self.state.config_revision = self.config.revision;
        self.state.source = self.config.source;
        self.state.wled_url = self
            .config
            .output
            .wled_url()
            .map(ToString::to_string)
            .unwrap_or_default();
        self.state.restart_required = self.config.web != self.active_web;
        tracing::info!(
            revision = self.config.revision.get(),
            restart_required = self.state.restart_required,
            "configuration saved"
        );
    }
    fn publish_state(&mut self) {
        self.state.pipeline = self.lighting.status.borrow().clone();
        if let Some(observed) = &mut self.state.observed {
            observed.pipeline_phase = self.state.pipeline.phase;
        }
        self.state.applied_revision = self.state.pipeline.applied_revision;
        self.state_tx.send_replace(self.state.clone());
    }
    fn record_observation(
        &mut self,
        revision: prysm_contract::ConfigRevision,
        snapshot: LightingSnapshot,
    ) {
        if revision == self.config.revision {
            update_observation(&mut self.state, snapshot);
            self.publish_state();
        }
    }
    fn ensure_available(&self) -> Result<(), ApiFailure> {
        if self.stopping {
            return Err(ApiFailure::unavailable());
        }
        if matches!(self.state.operation, OperationStatus::Pending { .. }) {
            return Err(ApiFailure::conflict("a change is already running"));
        }
        Ok(())
    }
    fn finish_operation(
        &mut self,
        id: OperationId,
        result: Result<()>,
        snapshot: LightingSnapshot,
        observations: &Observations,
    ) {
        self.state.operation = match result {
            Ok(()) => {
                tracing::info!(operation_id = ?id, "operation completed");
                OperationStatus::Succeeded { id }
            }
            Err(error) => failed_operation(id, &error),
        };
        if !observations.has_changed().unwrap_or(true) {
            update_observation(&mut self.state, snapshot);
        }
        self.publish_state();
    }
    async fn apply_config(
        lighting: &LightingService,
        previous: &Config,
        config: &Config,
    ) -> Result<()> {
        let running = matches!(
            lighting.status.borrow().phase,
            PipelinePhase::Starting | PipelinePhase::Running
        );
        let source_changed = previous.source != config.source;
        let power = if source_changed {
            lighting.observe().await?.power
        } else {
            running
        };
        let reconnect =
            PipelineConfig::from(previous).requires_restart(&PipelineConfig::from(config));
        if running && reconnect {
            lighting.capture(CaptureAction::Stop).await?;
        }
        lighting
            .pipeline
            .configure(PipelineConfig::from(config))
            .await?;
        if (running && reconnect) || source_changed {
            lighting
                .switch(Target {
                    power,
                    source: config.source,
                })
                .await?;
        }
        Ok(())
    }
    pub(crate) fn spawn(
        path: PathBuf,
        config: &Config,
        lighting: Arc<LightingService>,
        config_tx: watch::Sender<Config>,
    ) -> (ControllerHandle, App) {
        let config_rx = config_tx.subscribe();
        let state = initial_state(config);
        let (state_tx, state_rx) = watch::channel(state.clone());
        let (commands, receiver) = mpsc::channel(32);
        let shutdown = CancellationToken::new();
        let controller = Self {
            path,
            active_web: config.web.clone(),
            config: config.clone(),
            config_tx,
            state,
            state_tx,
            stopping: false,
            lighting,
        };
        let task = tokio::spawn(controller.run(receiver, shutdown.clone()));
        (
            ControllerHandle { shutdown, task },
            App {
                commands,
                config: config_rx,
                state: state_rx,
            },
        )
    }

    fn begin_operation(&mut self) -> OperationId {
        let id = OperationId::new();
        self.state.operation = OperationStatus::Pending { id };
        self.publish_state();
        id
    }

    fn accept(
        &mut self,
        command: Command,
        observations: &Observations,
    ) -> Option<JoinHandle<Finished>> {
        match command {
            Command::SetTarget(target, reply) => {
                let available = self.ensure_available();
                if let Err(error) = available {
                    let _ = reply.send(Err(error));
                    return None;
                }
                let id = self.begin_operation();
                tracing::info!(operation_id = ?id, ?target, "lighting change requested");
                let _ = reply.send(Ok(OperationResponse { operation_id: id }));
                let lighting = self.lighting.clone();
                let previous = self.config.clone();
                let path = self.path.clone();
                let mut observations = observations.clone();
                Some(tokio::spawn(async move {
                    let mut saved = None;
                    let result = async {
                        lighting.switch(target).await?;
                        let mut config = previous.clone();
                        if config.source != target.source {
                            config.source = target.source;
                            config = Self::save(path, config)
                                .await
                                .context("lights changed but source could not be saved")?;
                        }
                        saved = Some(config.clone());
                        lighting
                            .pipeline
                            .configure(PipelineConfig::from(&config))
                            .await?;
                        Ok(())
                    }
                    .await;
                    observations.borrow_and_update();
                    let snapshot = lighting.snapshot().await;
                    Finished::Target {
                        id,
                        saved,
                        result,
                        snapshot,
                        observations,
                    }
                }))
            }
            Command::SetConfig(update, reply) => {
                let validation = self.ensure_available().and_then(|()| {
                    if update.expected_revision != self.config.revision
                        || update.config.revision != update.expected_revision
                    {
                        return Err(ApiFailure::conflict(
                            "configuration changed; refresh before saving",
                        ));
                    }
                    crate::validate_config(&update.config).map_err(|error| {
                        ApiFailure::new(
                            StatusCode::UNPROCESSABLE_ENTITY,
                            ErrorCode::InvalidRequest,
                            error.to_string(),
                        )
                    })
                });
                if let Err(error) = validation {
                    let _ = reply.send(Err(error));
                    return None;
                }
                let id = self.begin_operation();
                tracing::info!(operation_id = ?id, expected_revision = update.expected_revision.get(), "configuration change requested");
                let previous = self.config.clone();
                let path = self.path.clone();
                Some(tokio::spawn(async move {
                    let result = Self::save(path, update.config).await;
                    Finished::Saved {
                        id,
                        previous,
                        result,
                        reply,
                    }
                }))
            }
        }
    }

    fn finished(
        &mut self,
        event: Finished,
        observations: &Observations,
    ) -> Option<JoinHandle<Finished>> {
        match event {
            Finished::Saved {
                id,
                previous,
                result,
                reply,
            } => match result {
                Ok(config) => {
                    self.record_saved_config(config.clone());
                    self.publish_state();
                    let _ = reply.send(Ok(config.clone()));
                    let lighting = self.lighting.clone();
                    let mut observations = observations.clone();
                    Some(tokio::spawn(async move {
                        let result = Self::apply_config(&lighting, &previous, &config).await;
                        observations.borrow_and_update();
                        let snapshot = lighting.snapshot().await;
                        Finished::Applied {
                            id,
                            result,
                            snapshot,
                            observations,
                        }
                    }))
                }
                Err(error) => {
                    self.state.operation = failed_operation(id, &error);
                    self.publish_state();
                    let _ = reply.send(Err(ApiFailure::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        ErrorCode::Persistence,
                        "could not save configuration",
                    )));
                    None
                }
            },
            Finished::Target {
                id,
                saved,
                result,
                snapshot,
                observations,
            } => {
                if let Some(config) = saved {
                    self.record_saved_config(config);
                }
                self.finish_operation(id, result, snapshot, &observations);
                None
            }
            Finished::Applied {
                id,
                result,
                snapshot,
                observations,
            } => {
                self.finish_operation(id, result, snapshot, &observations);
                None
            }
        }
    }

    async fn run(
        mut self,
        mut commands: mpsc::Receiver<Command>,
        shutdown: CancellationToken,
    ) -> Result<()> {
        let mut operation: Option<JoinHandle<Finished>> = None;
        let lighting = self.lighting.clone();
        let (updates, mut observations) = watch::channel(None);
        let observer = lighting.observations(updates);
        tokio::pin!(observer);
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            if self.stopping && operation.is_none() {
                break;
            }
            tokio::select! {
                () = shutdown.cancelled(), if !self.stopping => {
                    self.stopping = true;
                    commands.close();
                }
                result = async { match operation.as_mut() {
                    Some(task) => task.await,
                    None => std::future::pending().await,
                }} => {
                    operation = self.finished(result.context("accepted operation task panicked")?, &observations);
                }
                command = commands.recv(), if !self.stopping => match command {
                    Some(command) => {
                        if let Some(task) = self.accept(command, &observations) { operation = Some(task); }
                    }
                    None => self.stopping = true,
                },
                () = &mut observer => {
                    anyhow::bail!("WLED observer stopped unexpectedly");
                }
                result = observations.changed() => {
                    result.context("WLED observation channel closed")?;
                    let update = observations.borrow_and_update().clone();
                    if let Some((revision, snapshot)) = update {
                        self.record_observation(revision, snapshot);
                    }
                }
                _ = tick.tick(), if !self.stopping => self.publish_state(),
            }
        }
        while let Ok(command) = commands.try_recv() {
            match command {
                Command::SetTarget(_, reply) => {
                    let _ = reply.send(Err(ApiFailure::unavailable()));
                }
                Command::SetConfig(_, reply) => {
                    let _ = reply.send(Err(ApiFailure::unavailable()));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::runtime::Pipeline;

    #[tokio::test]
    async fn retired_observations_cannot_replace_current_config_or_pipeline_status() {
        let config: Config = toml::from_str(include_str!("../../../../prysm.toml")).unwrap();
        let (pipeline, _) = Pipeline::spawn(PipelineConfig::from(&config)).unwrap();
        let (config_tx, config_rx) = watch::channel(config.clone());
        let mut state = initial_state(&config);
        state.pipeline.phase = PipelinePhase::Failed;
        state.pipeline.error = Some("current run failed".into());
        let (_, status) = watch::channel(state.pipeline.clone());
        let lighting = Arc::new(LightingService::new(config_rx, pipeline.clone(), status).unwrap());
        let (state_tx, state_rx) = watch::channel(state.clone());
        let mut controller = Controller {
            path: PathBuf::new(),
            active_web: config.web.clone(),
            config: config.clone(),
            config_tx,
            state,
            state_tx,
            stopping: false,
            lighting,
        };
        let retired_snapshot = || LightingSnapshot {
            observed: None,
            error: Some("old device error".into()),
            pipeline: PipelineStatus {
                phase: PipelinePhase::Running,
                applied_revision: Some(config.revision),
                frames_processed: 10,
                error: None,
            },
        };
        controller.record_observation(config.revision, retired_snapshot());
        assert_eq!(state_rx.borrow().pipeline.phase, PipelinePhase::Failed);
        assert_eq!(
            state_rx.borrow().pipeline.error.as_deref(),
            Some("current run failed")
        );
        controller.config.revision = config.revision.next().unwrap();
        controller.state.observation_error = None;
        controller.publish_state();
        controller.record_observation(config.revision, retired_snapshot());
        assert!(
            state_rx.borrow().observation_error.is_none(),
            "old config observations must be ignored"
        );
        pipeline.shutdown().await.unwrap();
    }
}
