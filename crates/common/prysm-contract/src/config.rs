//! Validated application configuration and HTTP update types.
use crate::{ConfigRevision, Input, Output, ProcessingSettings, Source};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Complete application configuration for a configured runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub revision: ConfigRevision,
    pub source: Source,
    pub input: Input,
    pub output: Output,
    pub processing: ProcessingSettings,
}
pub(crate) fn deserialize_http_base_url<'de, D: serde::Deserializer<'de>>(
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
impl Input {
    pub fn validate(&self) -> anyhow::Result<()> {
        if let Self::Webos {
            refresh_rate_hz,
            capture_resolution,
            ..
        } = self
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
        Ok(())
    }
}

impl Output {
    pub fn validate(&self) -> anyhow::Result<()> {
        // Revalidate programmatically constructed URLs as well as deserialized URLs.
        if let Some(url) = self.wled_url() {
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

impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        self.input.validate()?;
        self.output.validate()
    }
}
