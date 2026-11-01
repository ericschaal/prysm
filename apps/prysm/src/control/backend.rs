//! Capture lifecycle and WLED control used by the switching logic.
use crate::actors::runtime::{ConfigurePipeline, Pipeline, StartPipeline, StopPipeline};
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
pub(crate) struct LightingService {
    pub(super) client: reqwest::Client,
    pub(super) config: watch::Receiver<Config>,
    pub(super) pipeline: Addr<Pipeline>,
    pub(super) status: watch::Receiver<PipelineStatus>,
    active_output: Output,
}
impl LightingService {
    pub fn new(
        config: watch::Receiver<Config>,
        pipeline: Addr<Pipeline>,
        status: watch::Receiver<PipelineStatus>,
    ) -> Result<Self> {
        let active_output = config.borrow().output.clone();
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
    pub(super) fn requires_restart(&self, output: &Output) -> bool {
        self.active_output.is_egui() != output.is_egui()
    }

    fn has_wled_control(&self) -> bool {
        !self.active_output.is_egui() && self.config.borrow().wled.is_some()
    }

    pub(super) async fn switch(&self, target: Target) -> Result<()> {
        if target.power && target.source == Source::Prysm {
            let config = crate::config::PipelineConfig::from(&*self.config.borrow());
            self.pipeline
                .send(ConfigurePipeline(config))
                .await
                .context("pipeline unavailable")??;
        }
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
    pub(super) async fn snapshot(&self) -> LightingSnapshot {
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
                    realtime: matches!(
                        pipeline.phase,
                        PipelinePhase::Running | PipelinePhase::Completed
                    ),
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
    use crate::config::PipelineConfig;

    #[actix::test]
    async fn webos_disconnect_reports_waiting_and_reconnect_restores_running() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let available = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = available.local_addr().unwrap();
        drop(available);
        let mut config: Config = toml::from_str(include_str!("../../../../prysm.toml")).unwrap();
        config.wled = None;
        config.input = prysm_contract::Input::Webos {
            listen: address,
            capture_resolution: [4, 2].try_into().unwrap(),
            refresh_rate_hz: 30.try_into().unwrap(),
        };
        let (pipeline, mut status) = Pipeline::spawn(PipelineConfig::from(&config)).unwrap();
        let (_, receiver) = watch::channel(config);
        let lighting = LightingService::new(receiver, pipeline, status.clone()).unwrap();
        lighting
            .switch(Target {
                power: true,
                source: Source::Prysm,
            })
            .await
            .unwrap();
        for count in 1..=2 {
            let mut tv = tokio::net::TcpStream::connect(address).await.unwrap();
            let hello = br#"{"type":"hello","version":3}"#;
            tv.write_all(&u32::try_from(hello.len()).unwrap().to_be_bytes())
                .await
                .unwrap();
            tv.write_all(hello).await.unwrap();
            let mut prefix = [0; 4];
            tv.read_exact(&mut prefix).await.unwrap();
            let mut settings = vec![0; u32::from_be_bytes(prefix) as usize];
            tv.read_exact(&mut settings).await.unwrap();
            let mut frame = b"PRY3".to_vec();
            for field in [4_u32, 2, 0, 0, 24] {
                frame.extend(field.to_be_bytes());
            }
            frame.extend([255; 24]);
            tv.write_all(&frame).await.unwrap();
            tokio::time::timeout(Duration::from_secs(2), async {
                while status.borrow().frames_processed < count
                    || status.borrow().phase != PipelinePhase::Running
                {
                    status.changed().await.unwrap();
                }
            })
            .await
            .unwrap();
            assert!(lighting.snapshot().await.observed.unwrap().realtime);
            drop(tv);
            tokio::time::timeout(Duration::from_secs(2), async {
                while status.borrow().phase != PipelinePhase::Starting {
                    status.changed().await.unwrap();
                }
            })
            .await
            .expect("disconnected input must report waiting");
            let snapshot = lighting.snapshot().await;
            assert!(!snapshot.observed.unwrap().realtime);
            assert!(
                snapshot
                    .pipeline
                    .error
                    .as_ref()
                    .is_some_and(|error| error.contains("waiting"))
            );
        }
        lighting.capture(CaptureAction::Stop).await.unwrap();
        assert_eq!(
            lighting.snapshot().await.pipeline.phase,
            PipelinePhase::Idle
        );
    }

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
        let (pipeline, status) = Pipeline::spawn(PipelineConfig::from(&config)).unwrap();
        let (_, receiver) = watch::channel(config);
        let lighting = LightingService::new(receiver, pipeline, status).unwrap();
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
