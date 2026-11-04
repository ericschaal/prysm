//! Typed capture settings shared by the TV service and its worker.
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum Source {
    Video,
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum Compression {
    None,
    Lz4,
    #[serde(rename = "h264-software")]
    H264Software,
    #[serde(rename = "h264-hardware")]
    H264Hardware,
}

/// Private worker startup configuration supplied by the supervisor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerConfig {
    pub instance_url: InstanceUrl,
    pub source: Source,
    pub compression: Compression,
    #[serde(default)]
    pub metrics: bool,
}

pub const WORKER_CONFIG_ENV: &str = "PRYSM_WORKER_CONFIG";
const MAX_WORKER_CONFIG_LEN: usize = 1024;

impl WorkerConfig {
    pub fn encode(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    pub fn decode(payload: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            payload.len() <= MAX_WORKER_CONFIG_LEN,
            "oversized worker configuration"
        );
        Ok(serde_json::from_str(payload)?)
    }
}

/// Canonical HTTP(S) origin used by both capture and calibration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct InstanceUrl(url::Url);

impl InstanceUrl {
    #[must_use]
    #[allow(
        clippy::missing_panics_doc,
        reason = "the private URL always has an HTTP(S) scheme"
    )]
    pub fn capture_url(&self) -> url::Url {
        let mut url = self.0.clone();
        let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
        url.set_scheme(scheme).expect("validated HTTP origin");
        url.set_path(super::CAPTURE_PATH);
        url
    }
}
impl FromStr for InstanceUrl {
    type Err = anyhow::Error;
    fn from_str(input: &str) -> anyhow::Result<Self> {
        use anyhow::ensure;
        let input = input.trim();
        ensure!(
            !input.is_empty()
                && input.len() <= 512
                && !input.contains(['\\', '@', '?', '#'])
                && !input
                    .chars()
                    .any(|ch| ch.is_whitespace() || ch.is_control()),
            "Enter a Prysm hostname or HTTP(S) origin"
        );
        let (_, authority) = input
            .split_once("://")
            .ok_or_else(|| anyhow::anyhow!("Use an HTTP or HTTPS instance URL"))?;
        let authority = authority.strip_suffix('/').unwrap_or(authority);
        ensure!(
            !authority.is_empty() && !authority.contains('/') && !authority.ends_with(':'),
            "Use an instance address without a path"
        );
        let url = url::Url::parse(input)?;
        ensure!(
            matches!(url.scheme(), "http" | "https") && url.has_host() && url.port() != Some(0),
            "Use an HTTP or HTTPS instance URL with a valid port"
        );
        Ok(Self(url))
    }
}
impl TryFrom<String> for InstanceUrl {
    type Error = anyhow::Error;
    fn try_from(value: String) -> anyhow::Result<Self> {
        value.parse()
    }
}
impl fmt::Display for InstanceUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.origin().ascii_serialization())
    }
}
impl From<InstanceUrl> for String {
    fn from(value: InstanceUrl) -> Self {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h264_compression_choices_have_explicit_names() {
        assert!(serde_json::from_str::<Compression>(r#""h264""#).is_err());
        for (name, compression) in [
            ("h264-software", Compression::H264Software),
            ("h264-hardware", Compression::H264Hardware),
        ] {
            assert_eq!(
                serde_json::from_str::<Compression>(&format!("\"{name}\"")).unwrap(),
                compression
            );
            assert_eq!(serde_json::to_value(compression).unwrap(), name);
            let config = WorkerConfig {
                instance_url: "http://127.0.0.1:8787".parse().unwrap(),
                source: Source::Video,
                compression,
                metrics: false,
            };
            assert_eq!(
                WorkerConfig::decode(&config.encode().unwrap()).unwrap(),
                config
            );
        }
    }

    #[test]
    fn worker_configuration_is_typed_bounded_and_round_trips() {
        let config = WorkerConfig {
            instance_url: "http://127.0.0.1:8787".parse().unwrap(),
            source: Source::Both,
            compression: Compression::H264Software,
            metrics: true,
        };
        assert_eq!(
            WorkerConfig::decode(&config.encode().unwrap()).unwrap(),
            config
        );
        let mut legacy = serde_json::to_value(config).unwrap();
        legacy.as_object_mut().unwrap().remove("metrics");
        assert!(!WorkerConfig::decode(&legacy.to_string()).unwrap().metrics);
        for payload in [
            r#"{"instance_url":"127.0.0.1:0","source":"both","compression":"h264-software"}"#,
            r#"{"instance_url":"http://127.0.0.1:8787","source":"ui","compression":"h264-software"}"#,
            r#"{"instance_url":"http://127.0.0.1:8787","source":"both","compression":"h264-software","args":[]}"#,
        ] {
            assert!(WorkerConfig::decode(payload).is_err());
        }
        assert!(WorkerConfig::decode(&" ".repeat(MAX_WORKER_CONFIG_LEN + 1)).is_err());
    }

    #[test]
    fn only_valid_http_origins_can_be_saved() {
        for (input, origin) in [
            ("http://prysm.local:8787", "http://prysm.local:8787"),
            ("http://prysm.local:80", "http://prysm.local"),
            ("http://[::1]:8787", "http://[::1]:8787"),
            ("http://[::1]:8080", "http://[::1]:8080"),
            ("http://192.168.1.10", "http://192.168.1.10"),
            ("https://Prysm.local/", "https://prysm.local"),
        ] {
            let value: InstanceUrl = input.parse().unwrap();
            assert_eq!(value.to_string(), origin);
            assert_eq!(
                value.capture_url().as_str(),
                format!("{}/api/capture", origin.replacen("http", "ws", 1))
            );
        }
        for input in [
            "",
            "ws://host",
            "http:///foo",
            "https://x/a/..",
            "host:0",
            "host:65536",
            "host:",
            "host?x",
            "user@host",
            "host#x",
        ] {
            assert!(input.parse::<InstanceUrl>().is_err(), "{input}");
        }
    }
}
