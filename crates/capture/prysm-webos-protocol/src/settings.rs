//! Typed capture settings shared by the TV service and its worker.
use serde::{Deserialize, Serialize};
use std::{fmt, net::SocketAddr, str::FromStr};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Video,
    Ui,
    Both,
}

impl Source {
    #[must_use]
    pub const fn includes_video(self) -> bool {
        matches!(self, Self::Video | Self::Both)
    }
    #[must_use]
    pub const fn includes_ui(self) -> bool {
        matches!(self, Self::Ui | Self::Both)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Compression {
    None,
    Lz4,
    #[serde(rename = "h264-software")]
    H264Software,
    #[serde(rename = "h264-hardware")]
    H264Hardware,
}

/// Private worker startup configuration supplied by the supervisor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerConfig {
    pub receiver: ReceiverAddress,
    pub source: Source,
    pub compression: Compression,
    #[serde(default)]
    pub metrics: bool,
}

pub const WORKER_CONFIG_ENV: &str = "PRYSM_WORKER_CONFIG";
const MAX_WORKER_CONFIG_LEN: usize = 1024;

impl WorkerConfig {
    pub fn encode(self) -> anyhow::Result<String> {
        Ok(serde_json::to_string(&self)?)
    }

    pub fn decode(payload: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            payload.len() <= MAX_WORKER_CONFIG_LEN,
            "oversized worker configuration"
        );
        Ok(serde_json::from_str(payload)?)
    }
}

/// A receiver IP address with a nonzero destination port.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ReceiverAddress(SocketAddr);

impl ReceiverAddress {
    #[must_use]
    pub const fn socket_addr(self) -> SocketAddr {
        self.0
    }
}

impl FromStr for ReceiverAddress {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let address: SocketAddr = value.parse().map_err(|_| {
            anyhow::anyhow!("Receiver must be an IP:port or [IPv6]:port, with port 1–65535")
        })?;
        anyhow::ensure!(address.port() != 0, "Receiver port must be 1–65535");
        Ok(Self(address))
    }
}

impl TryFrom<String> for ReceiverAddress {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl fmt::Display for ReceiverAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<ReceiverAddress> for String {
    fn from(value: ReceiverAddress) -> Self {
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
                receiver: "127.0.0.1:19400".parse().unwrap(),
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
            receiver: "127.0.0.1:19400".parse().unwrap(),
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
            r#"{"receiver":"127.0.0.1:0","source":"both","compression":"h264-software"}"#,
            r#"{"receiver":"127.0.0.1:19400","source":"unknown","compression":"h264-software"}"#,
            r#"{"receiver":"127.0.0.1:19400","source":"both","compression":"h264-software","args":[]}"#,
        ] {
            assert!(WorkerConfig::decode(payload).is_err());
        }
        assert!(WorkerConfig::decode(&" ".repeat(MAX_WORKER_CONFIG_LEN + 1)).is_err());
    }

    #[test]
    fn receiver_address_parsing_and_json_preserve_the_nonzero_port_invariant() {
        for value in ["127.0.0.1:19400", "[::1]:19400"] {
            let address: ReceiverAddress = value.parse().unwrap();
            assert_eq!(address.to_string(), value);
            assert_eq!(serde_json::to_value(address).unwrap(), value);
            assert_eq!(
                serde_json::from_value::<ReceiverAddress>(value.into()).unwrap(),
                address
            );
        }
        for value in [
            "",
            "example.com:19400",
            "1.2.3.4:0",
            "1.2.3.4:65536",
            "::1:19400",
            "1.2.3.4:1;reboot",
        ] {
            assert!(value.parse::<ReceiverAddress>().is_err(), "{value}");
            assert!(
                serde_json::from_value::<ReceiverAddress>(value.into()).is_err(),
                "{value}"
            );
        }
    }
}
