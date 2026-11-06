//! WLED wire details. The owner supplies ordering; transport never retries writes.
use crate::WledSettings;
use prysm_contract::ControlError;
use prysm_core::EdgeColors;
use prysm_renderer_wled::WledRenderer;
use serde_json::{Value, json};
use std::{future::Future, net::SocketAddr, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Release,
    Realtime,
    Off,
    Native,
}
pub(super) struct WriteError {
    pub error: ControlError,
    pub uncertain: bool,
}
impl WriteError {
    fn uncertain(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            error: ControlError::Adapter(Box::new(error)),
            uncertain: true,
        }
    }
    fn rejected(message: String) -> Self {
        Self {
            error: ControlError::Adapter(Box::new(std::io::Error::other(message))),
            uncertain: false,
        }
    }
}

/// Private seam for deterministic delayed-write tests, not a public output backend API.
pub(super) trait Wire: Send + 'static {
    fn configure(&mut self, settings: &WledSettings) -> Result<(), ControlError>;
    fn mode(&mut self, mode: Mode) -> impl Future<Output = Result<(), WriteError>> + Send;
    fn colors(
        &mut self,
        colors: &EdgeColors,
    ) -> impl Future<Output = Result<(), ControlError>> + Send;
}

struct Target {
    url: String,
    renderer: WledRenderer,
    leds: [usize; 4],
}
pub(super) struct Network {
    client: reqwest::Client,
    target: Option<Target>,
    http_port: u16,
    ddp_port: u16,
}
impl Network {
    pub fn new() -> Result<Self, ControlError> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .build()
            .map_err(|error| ControlError::Adapter(Box::new(error)))?;
        Ok(Self {
            client,
            target: None,
            http_port: 80,
            ddp_port: 4048,
        })
    }
    #[cfg(test)]
    pub(super) fn local(http_port: u16, ddp_port: u16) -> Self {
        Self {
            http_port,
            ddp_port,
            ..Self::new().unwrap()
        }
    }
    async fn post(&self, value: Value) -> Result<(), WriteError> {
        let url = &self.target.as_ref().expect("configured WLED").url;
        let mut response = self
            .client
            .post(url)
            .json(&value)
            .send()
            .await
            .map_err(|error| WriteError {
                uncertain: !error.is_connect(),
                error: ControlError::Adapter(Box::new(error)),
            })?;
        let status = response.status();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(WriteError::uncertain)? {
            if body.len() + chunk.len() > 4096 {
                return Err(WriteError::uncertain(std::io::Error::other(
                    "WLED response exceeds 4096 bytes",
                )));
            }
            body.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            return Err(WriteError::rejected(format!(
                "WLED rejected mode write: HTTP {status}"
            )));
        }
        let reply: Value = serde_json::from_slice(&body).map_err(WriteError::uncertain)?;
        if let Some(error) = reply.get("error") {
            return Err(WriteError::rejected(format!(
                "WLED rejected mode write: {error}"
            )));
        }
        match reply.get("success").and_then(Value::as_bool) {
            Some(true) => Ok(()),
            Some(false) => Err(WriteError::rejected("WLED rejected mode write".into())),
            None => Err(WriteError::uncertain(std::io::Error::other(
                "WLED response did not acknowledge mode write",
            ))),
        }
    }
}
impl Wire for Network {
    fn configure(&mut self, settings: &WledSettings) -> Result<(), ControlError> {
        settings.validate()?;
        let renderer = WledRenderer::new(SocketAddr::new(settings.address, self.ddp_port))
            .map_err(|error| ControlError::Adapter(Box::new(error)))?;
        self.target = Some(Target {
            url: format!(
                "http://{}/json/state",
                SocketAddr::new(settings.address, self.http_port)
            ),
            renderer,
            leds: settings
                .leds
                .map(|count| usize::try_from(count).expect("validated LED count")),
        });
        Ok(())
    }
    async fn mode(&mut self, mode: Mode) -> Result<(), WriteError> {
        // A previous main-segment realtime session can ignore lor until released.
        let value = match mode {
            Mode::Release => json!({"live":false}),
            Mode::Realtime => json!({"on":true,"lor":0,"live":true,"tt":0}),
            Mode::Off => json!({"on":false,"lor":2,"tt":0}),
            Mode::Native => json!({"on":true,"lor":2,"tt":0}),
        };
        self.post(value).await
    }
    async fn colors(&mut self, colors: &EdgeColors) -> Result<(), ControlError> {
        let target = self.target.as_mut().expect("configured WLED");
        target
            .renderer
            .render_edges(colors, target.leds)
            .map_err(|error| ControlError::Adapter(Box::new(error)))
    }
}
