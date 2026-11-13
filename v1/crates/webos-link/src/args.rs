//! Worker argv. The supervisor builds it with `to_args`; the worker parses it
//! with `parse`; a rejection is a packaging bug (exit `USAGE`).
use prysm_ports::LightingTarget;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::str::FromStr;

/// `prysm-webos --settings PATH --listen ADDR --lighting off|processed|device_native`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullArgs {
    /// The engine's settings file.
    pub settings: PathBuf,
    /// Portal listen address, `0.0.0.0:8787` on the TV.
    pub listen: SocketAddr,
    /// Initial lighting target.
    pub lighting: LightingTarget,
}

/// `prysm-webos-slim --receiver HOST:PORT`. What to capture and how to
/// encode it comes from the receiver (its stream request), not from the TV.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlimArgs {
    /// The headless host's frame receiver.
    pub receiver: ReceiverAddress,
}

/// Where a slim worker sends frames: `host:port`, the host an IP address
/// (IPv6 in brackets) or a DNS name such as `prysm.local`, the port fixed.
/// Only the syntax is checked here; the worker resolves a name at every
/// connection attempt, so a changed address is picked up on reconnect.
/// Serialized as the `host:port` string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ReceiverAddress {
    /// An IP address or a host name, without brackets.
    host: String,
    port: u16,
}

impl ReceiverAddress {
    /// The IP address or host name, without IPv6 brackets.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The address itself when the host is an IP address: nothing to resolve.
    #[must_use]
    pub fn socket_addr(&self) -> Option<SocketAddr> {
        let ip: IpAddr = self.host.parse().ok()?;
        Some(SocketAddr::new(ip, self.port))
    }
}

impl std::fmt::Display for ReceiverAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

impl FromStr for ReceiverAddress {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        const EXAMPLE: &str = "such as 192.168.1.20:7380 or prysm.local:7380";
        let (host, port) = text
            .rsplit_once(':')
            .ok_or_else(|| format!("needs host:port, {EXAMPLE}"))?;
        let port = match port.parse::<u16>() {
            Ok(port) if port != 0 => port,
            _ => return Err(format!("needs a fixed port, {EXAMPLE}")),
        };
        let ip = match host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
            Some(v6) => Some(IpAddr::V6(
                v6.parse::<Ipv6Addr>()
                    .map_err(|_| format!("`{v6}` is not an IPv6 address"))?,
            )),
            None => host.parse::<Ipv4Addr>().ok().map(IpAddr::V4),
        };
        let host = match ip {
            Some(ip) if ip.is_unspecified() => {
                return Err(format!("needs the receiver's address, not {ip}"));
            }
            Some(ip) => ip.to_string(),
            None if is_host_name(host) => host.to_owned(),
            None => return Err(format!("`{host}` is not a host name or IP address")),
        };
        Ok(Self { host, port })
    }
}

/// A DNS name: dot-separated labels of letters, digits and inner hyphens,
/// at most 253 characters; the last label not all digits (that would be a
/// malformed IPv4 address).
fn is_host_name(host: &str) -> bool {
    let label = |label: &str| {
        (1..=63).contains(&label.len())
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    };
    host.len() <= 253
        && host.split('.').all(label)
        && host
            .rsplit('.')
            .next()
            .is_some_and(|last| !last.bytes().all(|b| b.is_ascii_digit()))
}

impl TryFrom<String> for ReceiverAddress {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        text.parse()
            .map_err(|reason| format!("receiver `{text}`: {reason}"))
    }
}

impl From<ReceiverAddress> for String {
    fn from(address: ReceiverAddress) -> Self {
        address.to_string()
    }
}

/// The encoding of the frames a slim worker sends: the one its receiver
/// requested. Exactly that encoder is used: one that cannot start or fails
/// is reported, never a switch to another.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlimEncoding {
    /// The TV's hardware H.264 encoder (about 2 Mbit/s).
    #[default]
    H264Hardware,
    /// OpenH264 on the TV's CPU (about 2 Mbit/s).
    H264Software,
    /// Uncompressed NV12 (320x180 at 30 fps is about 21 Mbit/s).
    Raw,
}

impl SlimEncoding {
    const ALL: [Self; 3] = [Self::H264Hardware, Self::H264Software, Self::Raw];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::H264Hardware => "h264_hardware",
            Self::H264Software => "h264_software",
            Self::Raw => "raw",
        }
    }
}

impl std::fmt::Display for SlimEncoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for SlimEncoding {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|encoding| encoding.name() == name)
            .ok_or_else(|| "expected h264_hardware, h264_software or raw".into())
    }
}

impl FullArgs {
    #[must_use]
    pub fn to_args(&self) -> Vec<String> {
        vec![
            "--settings".into(),
            self.settings.display().to_string(),
            "--listen".into(),
            self.listen.to_string(),
            "--lighting".into(),
            self.lighting.as_str().into(),
        ]
    }

    /// # Errors
    /// A missing, repeated, unknown or invalid option.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = Options::parse(args, &["settings", "listen", "lighting"])?;
        Ok(Self {
            settings: PathBuf::from(options.take("settings")?),
            listen: options.value("listen")?,
            lighting: target(&options.take("lighting")?)?,
        })
    }
}

impl SlimArgs {
    #[must_use]
    pub fn to_args(&self) -> Vec<String> {
        vec!["--receiver".into(), self.receiver.to_string()]
    }

    /// # Errors
    /// A missing, repeated, unknown or invalid option.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = Options::parse(args, &["receiver"])?;
        Ok(Self {
            receiver: options.value("receiver")?,
        })
    }
}

/// `--lighting` values are the serde names of [`LightingTarget`].
fn target(name: &str) -> Result<LightingTarget, String> {
    LightingTarget::ALL
        .into_iter()
        .find(|target| target.as_str() == name)
        .ok_or_else(|| format!("--lighting: unknown target `{name}`"))
}

/// `--key value` pairs; every key in `keys` exactly once.
struct Options(BTreeMap<String, String>);

impl Options {
    fn parse(args: &[String], keys: &[&str]) -> Result<Self, String> {
        let mut values = BTreeMap::new();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let key = arg
                .strip_prefix("--")
                .filter(|key| keys.contains(key))
                .ok_or_else(|| format!("unexpected argument `{arg}`"))?;
            let value = args.next().ok_or_else(|| format!("{arg} needs a value"))?;
            if values.insert(key.to_owned(), value.clone()).is_some() {
                return Err(format!("{arg} given twice"));
            }
        }
        Ok(Self(values))
    }

    fn take(&mut self, key: &str) -> Result<String, String> {
        self.0
            .remove(key)
            .ok_or_else(|| format!("--{key} is required"))
    }

    fn value<T: FromStr>(&mut self, key: &str) -> Result<T, String>
    where
        T::Err: std::fmt::Display,
    {
        let value = self.take(key)?;
        value
            .parse()
            .map_err(|error| format!("--{key} `{value}`: {error}"))
    }
}
