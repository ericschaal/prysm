//! Capture choices shared by the TV app's configuration and capture CLI.
use serde::{Deserialize, Serialize};
use std::{fmt, net::SocketAddr, str::FromStr};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Video,
    Ui,
    Both,
}

impl Source {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Ui => "ui",
            Self::Both => "both",
        }
    }
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
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
#[serde(rename_all = "lowercase")]
pub enum Compression {
    None,
    Lz4,
    H264,
}

impl Compression {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Lz4 => "lz4",
            Self::H264 => "h264",
        }
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
