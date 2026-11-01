//! Own the active session and serialize lifecycle/configuration commands.
use super::{
    pipeline::{self, PipelineExit, Renderers},
    processor::ConfigureProcessor,
};
use crate::{config::PipelineConfig, input::InputSource};
use actix::{
    Actor, ActorFutureExt, Addr, AtomicResponse, Context, Handler, Message, Recipient, WrapFuture,
};
use anyhow::{Context as _, Result, bail};
use prysm_contract::{PipelinePhase, PipelineStatus};
use tokio::{sync::watch, task::JoinHandle};
use tokio_util::sync::DropGuard;

pub struct Pipeline {
    config: PipelineConfig,
    status: watch::Sender<PipelineStatus>,
    active: Option<ActiveSession>,
    renderers: Renderers,
    input: InputSource,
}

struct ActiveSession {
    shutdown: DropGuard,
    processor: Recipient<ConfigureProcessor>,
    task: JoinHandle<Result<PipelineExit>>,
}

#[derive(Message)]
#[rtype(result = "Result<()>")]
pub struct StartPipeline;
#[derive(Message)]
#[rtype(result = "Result<()>")]
pub struct StopPipeline;
#[derive(Message)]
#[rtype(result = "Result<()>")]
pub struct ConfigurePipeline(pub PipelineConfig);

pub(super) fn idle() -> PipelineStatus {
    PipelineStatus {
        phase: PipelinePhase::Idle,
        applied_revision: None,
        frames_processed: 0,
        error: None,
    }
}

impl Pipeline {
    pub fn spawn(config: PipelineConfig) -> Result<(Addr<Self>, watch::Receiver<PipelineStatus>)> {
        Self::with_renderers(config, Renderers::default())
    }

    pub(crate) fn with_renderers(
        config: PipelineConfig,
        renderers: Renderers,
    ) -> Result<(Addr<Self>, watch::Receiver<PipelineStatus>)> {
        let input = InputSource::new(config.input.clone())?;
        let (status, receiver) = watch::channel(idle());
        Ok((
            Self {
                config,
                status,
                active: None,
                renderers,
                input,
            }
            .start(),
            receiver,
        ))
    }
}

impl Actor for Pipeline {
    type Context = Context<Self>;
}

impl Handler<StartPipeline> for Pipeline {
    type Result = AtomicResponse<Self, Result<()>>;
    fn handle(&mut self, _: StartPipeline, _: &mut Self::Context) -> Self::Result {
        if self
            .active
            .as_ref()
            .is_some_and(|active| !active.task.is_finished())
        {
            return AtomicResponse::new(Box::pin(async { Ok(()) }.into_actor(self)));
        }
        let config = self.config.clone();
        let status = self.status.clone();
        let renderers = self.renderers.clone();
        let input = self.input.clone();
        status.send_replace(PipelineStatus {
            phase: PipelinePhase::Starting,
            applied_revision: Some(config.revision),
            ..idle()
        });
        AtomicResponse::new(Box::pin(
            async move {
                let session = pipeline::start(config, status.clone(), renderers, input).await?;
                let shutdown = session.shutdown.clone().drop_guard();
                let processor = session.processor.clone();
                let task = actix::spawn(async move {
                    let result = session.wait().await;
                    status.send_modify(|state| match &result {
                        Ok(PipelineExit::Completed) => state.phase = PipelinePhase::Completed,
                        Ok(PipelineExit::Stopped) => state.phase = PipelinePhase::Idle,
                        Err(error) => {
                            state.phase = PipelinePhase::Failed;
                            state.error = Some(format!("{error:#}"));
                        }
                    });
                    result
                });
                Ok::<_, anyhow::Error>(ActiveSession {
                    shutdown,
                    processor,
                    task,
                })
            }
            .into_actor(self)
            .map(|result, actor, _| match result {
                Ok(active) => {
                    actor.active = Some(active);
                    Ok(())
                }
                Err(error) => {
                    actor.status.send_modify(|state| {
                        state.phase = PipelinePhase::Failed;
                        state.error = Some(format!("{error:#}"));
                    });
                    Err(error)
                }
            }),
        ))
    }
}

impl Handler<StopPipeline> for Pipeline {
    type Result = AtomicResponse<Self, Result<()>>;
    fn handle(&mut self, _: StopPipeline, _: &mut Self::Context) -> Self::Result {
        let active = self.active.take();
        AtomicResponse::new(Box::pin(
            async move {
                if let Some(active) = active {
                    active.shutdown.disarm().cancel();
                    // Session errors are already observed; joining still guarantees output has stopped.
                    let _ = active.task.await.context("pipeline task panicked")?;
                }
                Ok(())
            }
            .into_actor(self)
            .map(|result, actor, _| {
                if result.is_ok() {
                    actor.status.send_replace(idle());
                }
                result
            }),
        ))
    }
}

impl Handler<ConfigurePipeline> for Pipeline {
    type Result = AtomicResponse<Self, Result<()>>;
    fn handle(&mut self, msg: ConfigurePipeline, _: &mut Self::Context) -> Self::Result {
        let config = msg.0;
        let input = self.input.clone();
        let processor = self
            .active
            .as_ref()
            .filter(|active| !active.task.is_finished())
            .map(|active| active.processor.clone());
        let restart = processor.is_some() && self.config.requires_restart(&config);
        AtomicResponse::new(Box::pin(
            async move {
                if restart {
                    bail!("stop the pipeline before changing input or output");
                }
                if let Some(processor) = processor {
                    processor
                        .send(ConfigureProcessor(config.processing, config.revision))
                        .await
                        .context("processor unavailable while applying settings")?;
                }
                let input = input.reconfigure(config.input.clone())?;
                Ok::<_, anyhow::Error>((config, input))
            }
            .into_actor(self)
            .map(|result, actor, _| {
                let (config, input) = result?;
                actor.config = config;
                actor.input = input;
                Ok(())
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[actix::test]
    async fn webos_listener_survives_pipeline_stop_and_configuration() {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let available = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = available.local_addr().unwrap();
            drop(available);
            let mut config: prysm_contract::Config =
                toml::from_str(include_str!("../../../../prysm.toml")).unwrap();
            config.wled = None;
            config.input = prysm_contract::Input::Webos {
                listen: address,
                capture_resolution: [4, 2].try_into().unwrap(),
                refresh_rate_hz: 30.try_into().unwrap(),
            };
            let pipeline_config = PipelineConfig::from(&config);
            let (pipeline, _) = Pipeline::spawn(pipeline_config.clone()).unwrap();
            async fn message(tv: &mut tokio::net::TcpStream) -> serde_json::Value {
                let mut prefix = [0; 4];
                tv.read_exact(&mut prefix).await.unwrap();
                let mut bytes = vec![0; u32::from_be_bytes(prefix) as usize];
                tv.read_exact(&mut bytes).await.unwrap();
                serde_json::from_slice(&bytes).unwrap()
            }
            for _ in 0..2 {
                let mut tv = tokio::net::TcpStream::connect(address).await.unwrap();
                let hello = br#"{"type":"hello","version":2}"#;
                tv.write_all(&(hello.len() as u32).to_be_bytes())
                    .await
                    .unwrap();
                tv.write_all(hello).await.unwrap();
                assert_eq!(message(&mut tv).await["type"], "idle");
                pipeline
                    .send(ConfigurePipeline(pipeline_config.clone()))
                    .await
                    .unwrap()
                    .unwrap();
                pipeline.send(StartPipeline).await.unwrap().unwrap();
                assert_eq!(message(&mut tv).await["type"], "capture_settings");
                pipeline.send(StopPipeline).await.unwrap().unwrap();
                assert_eq!(tv.read(&mut [0]).await.unwrap(), 0);
            }
        })
        .await
        .unwrap();
    }
}
