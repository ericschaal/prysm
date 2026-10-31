//! Validated application configuration and HTTP update types.
use crate::{ConfigRevision, Input, Output, ProcessingSettings, Source};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use utoipa::ToSchema;

/// The complete saved configuration. Runtime observations and power are never saved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub revision: ConfigRevision,
    pub source: Source,
    pub web: WebConfig,
    pub input: Input,
    pub output: Output,
    pub wled: Option<WledConfig>,
    pub processing: ProcessingSettings,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateConfig {
    pub expected_revision: ConfigRevision,
    pub config: Config,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WebConfig {
    #[schema(value_type = String)]
    pub listen: SocketAddr,
    pub allowed_hosts: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WledConfig {
    #[serde(deserialize_with = "deserialize_http_base_url")]
    #[schema(value_type = String)]
    pub url: url::Url,
}
fn deserialize_http_base_url<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<url::Url, D::Error> {
    let value = url::Url::deserialize(deserializer)?;
    if !matches!(value.scheme(), "http" | "https")
        || value.host_str().is_none()
        || !value.username().is_empty()
        || value.password().is_some()
        || value.path() != "/"
        || value.query().is_some()
        || value.fragment().is_some()
    {
        return Err(serde::de::Error::custom(
            "WLED URL must be an HTTP base URL",
        ));
    }
    Ok(value)
}
impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.web.allowed_hosts.is_empty()
                && self
                    .web
                    .allowed_hosts
                    .iter()
                    .all(|host| !host.trim().is_empty()),
            "configure at least one allowed panel host"
        );
        anyhow::ensure!(
            self.source == Source::Prysm || (!self.output.is_egui() && self.wled.is_some()),
            "WLED effects require WLED output and a [wled] HTTP endpoint"
        );
        if let Input::Webos {
            refresh_rate_hz,
            capture_resolution,
            ..
        } = self.input
        {
            let [width, height] = capture_resolution.dimensions();
            anyhow::ensure!(
                (2..=1920).contains(&width)
                    && width.is_multiple_of(2)
                    && (2..=1080).contains(&height)
                    && height.is_multiple_of(2),
                "webOS capture_resolution must have even dimensions within 2–1920 × 2–1080"
            );
            anyhow::ensure!(
                refresh_rate_hz.get() <= 60,
                "webOS refresh_rate_hz must be between 1 and 60"
            );
        }
        // Revalidate programmatically constructed URLs as well as deserialized URLs.
        if let Some(wled) = &self.wled {
            let url = &wled.url;
            anyhow::ensure!(
                matches!(url.scheme(), "http" | "https")
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.path() == "/"
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "WLED URL must be an HTTP base URL"
            );
        }
        Ok(())
    }
}
