//! Capture lifecycle and WLED control used by the switching logic.
use crate::actors::{
    pipeline::Renderers,
    runtime::{Pipeline, StartPipeline, StopPipeline},
};
use crate::config::PipelineConfig;
use actix::Addr;
use anyhow::{Context, Result, bail};
use prysm_contract::{
    Config, Observation, Output, PipelinePhase, PipelineStatus, Source, Target, WledStatus,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::sync::watch;

#[derive(Clone, Copy)]
pub(super) enum CaptureAction {
    Start,
    Stop,
}
#[derive(Debug, Clone, Copy, Serialize)]
pub(super) struct WledCommand {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lor: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tt: Option<u8>,
}
impl WledCommand {
    pub const SUPPRESS_AND_OFF: Self = Self {
        on: Some(false),
        live: Some(false),
        lor: Some(2),
        tt: Some(0),
    };
    pub const RELEASE: Self = Self {
        on: None,
        live: Some(false),
        lor: None,
        tt: None,
    };
    pub const OFF: Self = Self {
        on: Some(false),
        live: None,
        lor: Some(2),
        tt: Some(0),
    };
    pub const EFFECTS: Self = Self {
        on: Some(true),
        live: None,
        lor: Some(2),
        tt: None,
    };
    pub const REALTIME: Self = Self {
        on: Some(true),
        live: None,
        lor: Some(0),
        tt: None,
    };
}
pub(super) trait LightingBackend {
    async fn capture(&self, action: CaptureAction) -> Result<()>;
    async fn wled(&self, value: WledCommand) -> Result<()>;
    async fn observe(&self) -> Result<Observation>;
}
#[derive(Clone)]
pub(super) struct LightingSnapshot {
    pub observed: Option<Observation>,
    pub pipeline: PipelineStatus,
    pub error: Option<String>,
}
pub(super) struct LightingService {
    pub client: reqwest::Client,
    pub config: watch::Receiver<Config>,
    pub pipeline: Addr<Pipeline>,
    pub status: watch::Receiver<PipelineStatus>,
    active_output: Output,
}
impl LightingService {
    pub fn new(config: watch::Receiver<Config>, renderers: Renderers) -> Result<Self> {
        let initial = PipelineConfig::from(&*config.borrow());
        let active_output = initial.output.clone();
        let (pipeline, status) = Pipeline::with_renderers(initial, renderers);
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(3))
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .build()?,
            active_output,
            config,
            pipeline,
            status,
        })
    }
    pub fn requires_restart(&self, output: &Output) -> bool {
        self.active_output.is_egui() != output.is_egui()
    }

    fn has_wled_control(&self) -> bool {
        !self.active_output.is_egui() && self.config.borrow().wled.is_some()
    }

    pub async fn switch(&self, target: Target) -> Result<()> {
        if self.has_wled_control() {
            return super::switching::switch(self, target).await;
        }
        anyhow::ensure!(
            target.source == Source::Prysm,
            "this output supports only screen sync"
        );
        self.capture(if target.power {
            CaptureAction::Start
        } else {
            CaptureAction::Stop
        })
        .await
    }

    fn url(&self) -> Result<reqwest::Url> {
        self.config
            .borrow()
            .wled
            .as_ref()
            .map(|wled| wled.url.clone())
            .context("no WLED HTTP endpoint configured")
    }
    async fn wled_status(&self) -> Result<WledStatus> {
        #[derive(Deserialize)]
        struct Document {
            state: State,
            info: Info,
        }
        #[derive(Deserialize)]
        struct State {
            on: bool,
            lor: u8,
        }
        #[derive(Deserialize)]
        struct Info {
            live: bool,
            lm: Option<String>,
        }
        let document: Document = self
            .client
            .get(self.url()?.join("json/si")?)
            .send()
            .await
            .context("cannot reach WLED")?
            .error_for_status()?
            .json()
            .await?;
        Ok(WledStatus {
            power: document.state.on,
            realtime: document.info.live,
            realtime_source: document.info.lm,
            live_override: document.state.lor,
        })
    }
    pub async fn snapshot(&self) -> LightingSnapshot {
        if !self.has_wled_control() {
            let pipeline = self.status.borrow().clone();
            let power = matches!(
                pipeline.phase,
                PipelinePhase::Starting | PipelinePhase::Running | PipelinePhase::Completed
            );
            return LightingSnapshot {
                observed: Some(Observation {
                    pipeline_phase: pipeline.phase,
                    power,
                    realtime: power,
                    realtime_source: None,
                    live_override: if power { 0 } else { 2 },
                }),
                pipeline,
                error: None,
            };
        }
        let wled = self.wled_status().await;
        let pipeline = self.status.borrow().clone();
        let (observed, error) = match wled {
            Ok(wled) => (
                Some(Observation {
                    pipeline_phase: pipeline.phase,
                    power: wled.power,
                    realtime: wled.realtime,
                    realtime_source: wled.realtime_source,
                    live_override: wled.live_override,
                }),
                None,
            ),
            Err(error) => (None, Some(format!("{error:#}"))),
        };
        LightingSnapshot {
            observed,
            pipeline,
            error,
        }
    }
}

impl LightingBackend for LightingService {
    async fn capture(&self, action: CaptureAction) -> Result<()> {
        match action {
            CaptureAction::Start => self
                .pipeline
                .send(StartPipeline)
                .await
                .context("pipeline unavailable")?,
            CaptureAction::Stop => self
                .pipeline
                .send(StopPipeline)
                .await
                .context("pipeline unavailable")?,
        }
    }
    async fn wled(&self, value: WledCommand) -> Result<()> {
        #[derive(Deserialize)]
        struct Reply {
            error: Option<u32>,
        }
        let reply: Reply = self
            .client
            .post(self.url()?.join("json/state")?)
            .json(&value)
            .send()
            .await
            .context("cannot reach WLED")?
            .error_for_status()?
            .json()
            .await?;
        if let Some(error) = reply.error {
            bail!("WLED rejected update: {error}");
        }
        Ok(())
    }
    async fn observe(&self) -> Result<Observation> {
        let snapshot = self.snapshot().await;
        snapshot
            .observed
            .with_context(|| snapshot.error.unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[actix::test]
    async fn ddp_without_http_uses_pipeline_state_and_reports_capture_failure() {
        let mut config: Config = toml::from_str(include_str!("../../../../prysm.toml")).unwrap();
        config.wled = None;
        config.input = prysm_contract::Input::File {
            capture_resolution: [640, 360].try_into().unwrap(),
            refresh_rate_hz: 30.try_into().unwrap(),
            path: "/missing/prysm-ddp-test.mp4".to_owned().try_into().unwrap(),
        };
        config.validate().unwrap();
        let (_, receiver) = watch::channel(config);
        let lighting = LightingService::new(receiver, Renderers::default()).unwrap();
        assert!(!lighting.snapshot().await.observed.unwrap().power);
        lighting
            .switch(Target {
                power: false,
                source: Source::Prysm,
            })
            .await
            .unwrap();
        let error = lighting
            .switch(Target {
                power: true,
                source: Source::Prysm,
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Cannot open video"));
        let snapshot = lighting.snapshot().await;
        assert_eq!(snapshot.pipeline.phase, PipelinePhase::Failed);
        assert!(snapshot.error.is_none());
        assert!(!snapshot.observed.unwrap().power);
        assert!(
            lighting
                .switch(Target {
                    power: true,
                    source: Source::Wled
                })
                .await
                .is_err()
        );
    }
}
