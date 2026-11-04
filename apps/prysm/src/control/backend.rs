//! Capture lifecycle and WLED control used by the switching logic.
use crate::pipeline::runtime::Pipeline;
use anyhow::{Context, Result, bail};
use prysm_contract::{Config, Observation, PipelineStatus};
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
    pub error: Option<String>,
}
#[derive(Deserialize)]
pub(super) struct WledStatus {
    state: WledState,
    info: WledInfo,
}
#[derive(Deserialize)]
struct WledState {
    on: bool,
    lor: u8,
}
#[derive(Deserialize)]
struct WledInfo {
    live: bool,
    lm: Option<String>,
}
pub(crate) struct LightingService {
    pub(super) client: reqwest::Client,
    pub(super) config: watch::Receiver<Config>,
    pub(super) pipeline: Pipeline,
    pub(super) status: watch::Receiver<PipelineStatus>,
}
impl LightingService {
    pub(super) fn client() -> Result<reqwest::Client> {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .context("cannot initialize WLED HTTP client")
    }

    pub fn new(
        client: reqwest::Client,
        config: watch::Receiver<Config>,
        pipeline: Pipeline,
        status: watch::Receiver<PipelineStatus>,
    ) -> Self {
        Self {
            client,
            config,
            pipeline,
            status,
        }
    }
    fn url(&self) -> Result<reqwest::Url> {
        self.config
            .borrow()
            .output
            .wled_url()
            .cloned()
            .context("no WLED HTTP endpoint configured")
    }
    pub(super) async fn wled_status(&self, url: &reqwest::Url) -> Result<WledStatus> {
        self.client
            .get(url.join("json/si")?)
            .send()
            .await
            .context("cannot reach WLED")?
            .error_for_status()?
            .json()
            .await
            .context("invalid WLED status")
    }
    pub(super) async fn snapshot(&self) -> LightingSnapshot {
        let wled = match self.url() {
            Ok(url) => self.wled_status(&url).await,
            Err(error) => Err(error),
        };
        self.status_snapshot(wled)
    }
    pub(super) fn status_snapshot(&self, wled: Result<WledStatus>) -> LightingSnapshot {
        let pipeline = self.status.borrow().clone();
        let (observed, error) = match wled {
            Ok(wled) => (
                Some(Observation {
                    pipeline_phase: pipeline.phase,
                    power: wled.state.on,
                    realtime: wled.info.live,
                    realtime_source: wled.info.lm,
                    live_override: wled.state.lor,
                }),
                None,
            ),
            Err(error) => (None, Some(format!("{error:#}"))),
        };
        LightingSnapshot { observed, error }
    }
}

impl LightingBackend for LightingService {
    async fn capture(&self, action: CaptureAction) -> Result<()> {
        match action {
            CaptureAction::Start => self.pipeline.start().await,
            CaptureAction::Stop => self.pipeline.stop().await,
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
    use prysm_contract::PipelinePhase;

    #[tokio::test]
    async fn webos_disconnect_reports_waiting_and_reconnect_restores_running() {
        use futures::{SinkExt, StreamExt};
        let webos = prysm_capture::WebosReceiver::new().unwrap();
        let mut config: Config = crate::test_support::config();
        let prysm_contract::Output::Wled { url, .. } = &mut config.output;
        *url = "http://127.0.0.1/".parse().unwrap();
        config.input = prysm_contract::Input::Webos {
            capture_resolution: [4, 2].try_into().unwrap(),
            refresh_rate_hz: 30.try_into().unwrap(),
        };
        let (pipeline, mut status) = Pipeline::with_preview(
            PipelineConfig::from(&config),
            webos.clone(),
            crate::preview::Preview::new().publisher(),
        );
        let (_, receiver) = watch::channel(config);
        let lighting = LightingService::new(
            LightingService::client().unwrap(),
            receiver,
            pipeline,
            status.clone(),
        );
        lighting.capture(CaptureAction::Start).await.unwrap();
        for count in 1..=2 {
            let mut tv = crate::test_support::tv_socket(&webos).await;
            assert!(matches!(
                prysm_webos_protocol::ControlMessage::decode(
                    &tv.next().await.unwrap().unwrap().into_data()
                )
                .unwrap(),
                prysm_webos_protocol::ControlMessage::CaptureSettings { .. }
            ));
            let mut frame = prysm_webos_protocol::Header {
                width: 4,
                height: 2,
                format: prysm_webos_protocol::PixelFormat::Rgb,
                codec: prysm_webos_protocol::Codec::Raw,
                payload_len: 24,
            }
            .encode()
            .unwrap()
            .to_vec();
            frame.extend([255; 24]);
            tv.send(tokio_tungstenite::tungstenite::Message::binary(frame))
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(2), async {
                while status.borrow().frames_processed < count
                    || status.borrow().phase != PipelinePhase::Running
                {
                    status.changed().await.unwrap();
                }
            })
            .await
            .unwrap();
            assert_eq!(status.borrow().phase, PipelinePhase::Running);
            drop(tv);
            tokio::time::timeout(Duration::from_secs(2), async {
                while status.borrow().phase != PipelinePhase::Starting {
                    status.changed().await.unwrap();
                }
            })
            .await
            .expect("disconnected input must report waiting");
            let snapshot = status.borrow().clone();
            assert!(
                snapshot
                    .error
                    .as_ref()
                    .is_some_and(|error| error.contains("waiting"))
            );
        }
        lighting.capture(CaptureAction::Stop).await.unwrap();
        assert_eq!(status.borrow().phase, PipelinePhase::Idle);
    }
}
