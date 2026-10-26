//! Serialize settings and lighting changes through one actor.
use super::{
    backend::{CaptureAction, LightingBackend, LightingService, LightingSnapshot},
    persistence,
};
use crate::actors::runtime::ConfigurePipeline;
use crate::config::PipelineConfig;
use actix::{
    Actor, ActorContext, ActorFutureExt, AsyncContext, AtomicResponse, Handler, Message,
    ResponseActFuture, StreamHandler, WrapFuture,
};
use anyhow::{Context as _, Result};
use axum::http::StatusCode;
use prysm_api::{ApiFailure, App, SetConfig, SetTarget};
use prysm_contract::{
    ApiError, Config, ErrorCode, OperationId, OperationResponse, OperationStatus, PipelinePhase,
    PipelineStatus, StateResponse, Target, WebConfig,
};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::watch;

#[derive(Message)]
#[rtype(result = "()")]
struct Finished(OperationId, Target, Result<()>);

#[derive(Message)]
#[rtype(result = "()")]
pub(super) struct Shutdown;

fn initial_state(config: &Config) -> StateResponse {
    StateResponse {
        source: config.source,
        observed: None,
        observation_error: None,
        operation: OperationStatus::Idle,
        wled_url: config
            .wled
            .as_ref()
            .map(|wled| wled.url.to_string())
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
    tracing::error!(error=%format!("{error:#}"),"operation failed");
    OperationStatus::Failed {
        id,
        error: ApiError {
            code: ErrorCode::Backend,
            message: format!("{error:#}"),
        },
    }
}
pub(super) struct Controller {
    path: PathBuf,
    config: Config,
    active_web: WebConfig,
    config_tx: watch::Sender<Config>,
    state: StateResponse,
    state_tx: watch::Sender<StateResponse>,
    stopping: bool,
    lighting: Arc<LightingService>,
}
impl Controller {
    async fn save(path: PathBuf, mut config: Config) -> Result<Config> {
        crate::validate_config(&config)?;
        config.revision = config.revision.next().map_err(anyhow::Error::msg)?;
        persistence::save(path, config.clone()).await?;
        Ok(config)
    }
    fn record_saved_config(&mut self, config: Config) {
        self.config = config;
        self.config_tx.send_replace(self.config.clone());
        self.state.config_revision = self.config.revision;
        self.state.source = self.config.source;
        self.state.wled_url = self
            .config
            .wled
            .as_ref()
            .map(|wled| wled.url.to_string())
            .unwrap_or_default();
        self.state.restart_required = self.config.web != self.active_web
            || self.lighting.requires_restart(&self.config.output);
    }
    fn publish_state(&self) {
        self.state_tx.send_replace(self.state.clone());
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
    ) {
        self.state.operation = match result {
            Ok(()) => OperationStatus::Succeeded { id },
            Err(error) => failed_operation(id, &error),
        };
        update_observation(&mut self.state, snapshot);
        self.publish_state();
    }
    async fn apply_config(
        lighting: &LightingService,
        previous: &Config,
        config: &Config,
    ) -> Result<()> {
        if lighting.requires_restart(&config.output) {
            return lighting.capture(CaptureAction::Stop).await;
        }
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
        let reconnect = PipelineConfig::from(previous)
            .requires_restart(&PipelineConfig::from(config))
            || previous.wled != config.wled;
        if running && reconnect {
            lighting.capture(CaptureAction::Stop).await?;
        }
        lighting
            .pipeline
            .send(ConfigurePipeline(PipelineConfig::from(config)))
            .await
            .context("pipeline unavailable")??;
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
}

impl Actor for Controller {
    type Context = actix::Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        ctx.set_mailbox_capacity(32);
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ctx.add_stream(futures::stream::unfold(
            (self.lighting.clone(), tick),
            |(lighting, mut tick)| async move {
                tick.tick().await;
                let snapshot = lighting.snapshot().await;
                Some((snapshot, (lighting, tick)))
            },
        ));
    }
}

impl StreamHandler<LightingSnapshot> for Controller {
    fn handle(&mut self, snapshot: LightingSnapshot, _: &mut Self::Context) {
        update_observation(&mut self.state, snapshot);
        self.publish_state();
    }
}

impl Handler<SetTarget> for Controller {
    type Result = Result<OperationResponse, ApiFailure>;

    fn handle(&mut self, SetTarget(target): SetTarget, ctx: &mut Self::Context) -> Self::Result {
        self.ensure_available()?;
        if self.lighting.requires_restart(&self.config.output) {
            return Err(ApiFailure::conflict(
                "restart the application to apply output settings",
            ));
        }
        if (self.config.output.is_egui() || self.config.wled.is_none())
            && target.source != prysm_contract::Source::Prysm
        {
            return Err(ApiFailure::conflict(
                "this output supports only screen sync",
            ));
        }
        let id = OperationId::new();
        self.state.operation = OperationStatus::Pending { id };
        self.publish_state();
        let lighting = self.lighting.clone();
        ctx.spawn(
            async move { lighting.switch(target).await }
                .into_actor(self)
                .map(move |result, _, ctx| ctx.notify(Finished(id, target, result))),
        );
        Ok(OperationResponse { operation_id: id })
    }
}

impl Handler<SetConfig> for Controller {
    type Result = AtomicResponse<Self, Result<Config, ApiFailure>>;

    fn handle(&mut self, SetConfig(update): SetConfig, _: &mut Self::Context) -> Self::Result {
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
            return AtomicResponse::new(Box::pin(async { Err(error) }.into_actor(self)));
        }
        let previous = self.config.clone();
        AtomicResponse::new(Box::pin(
            Self::save(self.path.clone(), update.config)
                .into_actor(self)
                .map(move |result, actor, ctx| {
                    let config = result.map_err(|error| {
                        tracing::error!(error=%format!("{error:#}"), "cannot save configuration");
                        ApiFailure::new(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            ErrorCode::Persistence,
                            "could not save configuration",
                        )
                    })?;
                    actor.record_saved_config(config.clone());
                    let id = OperationId::new();
                    actor.state.operation = OperationStatus::Pending { id };
                    actor.publish_state();
                    let lighting = actor.lighting.clone();
                    let applied = config.clone();
                    ctx.spawn(
                        async move {
                            let result = Self::apply_config(&lighting, &previous, &applied).await;
                            (result, lighting.snapshot().await)
                        }
                        .into_actor(actor)
                        .map(move |(result, snapshot), actor, _| {
                            actor.finish_operation(id, result, snapshot);
                        }),
                    );
                    Ok(config)
                }),
        ))
    }
}

impl Handler<Finished> for Controller {
    type Result = AtomicResponse<Self, ()>;

    fn handle(
        &mut self,
        Finished(id, target, result): Finished,
        _: &mut Self::Context,
    ) -> Self::Result {
        let mut config = self.config.clone();
        let source_changed = config.source != target.source;
        config.source = target.source;
        let path = self.path.clone();
        AtomicResponse::new(Box::pin(
            async move {
                result?;
                if source_changed {
                    Self::save(path, config)
                        .await
                        .context("lights changed but source could not be saved")
                } else {
                    Ok(config)
                }
            }
            .into_actor(self)
            .then(move |result, actor, _| {
                let result = result.map(|config| actor.record_saved_config(config));
                let lighting = actor.lighting.clone();
                let config = PipelineConfig::from(&actor.config);
                async move {
                    let configured = lighting
                        .pipeline
                        .send(ConfigurePipeline(config))
                        .await
                        .context("pipeline unavailable")
                        .and_then(|result| result);
                    let result = match configured {
                        Ok(()) => result,
                        Err(error) => Err(error),
                    };
                    (result, lighting.snapshot().await)
                }
                .into_actor(actor)
            })
            .map(move |(result, snapshot), actor, _| actor.finish_operation(id, result, snapshot)),
        ))
    }
}

impl Handler<Shutdown> for Controller {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, _: Shutdown, _: &mut Self::Context) -> Self::Result {
        self.stopping = true;
        let mut state = self.state_tx.subscribe();
        Box::pin(
            async move {
                while matches!(state.borrow().operation, OperationStatus::Pending { .. }) {
                    if state.changed().await.is_err() {
                        break;
                    }
                }
            }
            .into_actor(self)
            .map(|(), _, ctx| ctx.stop()),
        )
    }
}

impl Handler<crate::actors::Stop> for Controller {
    type Result = ();

    fn handle(&mut self, _: crate::actors::Stop, ctx: &mut Self::Context) {
        ctx.stop();
    }
}

impl Controller {
    pub(super) fn spawn(
        path: PathBuf,
        config: &Config,
        lighting: Arc<LightingService>,
        config_tx: watch::Sender<Config>,
    ) -> (actix::Addr<Self>, App) {
        let config_rx = config_tx.subscribe();
        let state = initial_state(config);
        let (state_tx, state_rx) = watch::channel(state.clone());
        let controller = Self {
            path,
            active_web: config.web.clone(),
            config: config.clone(),
            config_tx,
            state,
            state_tx,
            stopping: false,
            lighting,
        }
        .start();
        let app = App {
            set_target: controller.clone().recipient(),
            set_config: controller.clone().recipient(),
            config: config_rx,
            state: state_rx,
        };
        (controller, app)
    }
}
