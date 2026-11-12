//! The device address: `host[:port]`, parsed at validation and resolved at open.
use prysm_ports::{Error, Result};
use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// WLED's HTTP API port when the address names none.
const DEFAULT_PORT: u16 = 80;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Host {
    Ip(IpAddr),
    /// DNS or mDNS name, such as `wled.local`.
    Name(String),
}

/// A parsed `address` setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceAddress {
    pub host: Host,
    /// HTTP API port. DDP always uses UDP 4048.
    pub port: u16,
}

impl DeviceAddress {
    /// # Errors
    /// The reason `text` is not `host`, `host:port` or `[ipv6]:port`.
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        if text.is_empty() {
            return Err("a WLED host name or IP address is required");
        }
        if text.chars().any(char::is_whitespace) {
            return Err("must not contain spaces");
        }
        if text
            .get(..8)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
        {
            return Err(
                "WLED serves plain HTTP: enter the host without `https://`, \
                 such as `wled.local` or `wled.local:8080`",
            );
        }
        if text.contains("://") {
            return Err("enter the host without a scheme such as `http://`, \
                 for example `wled.local:8080`");
        }
        if text.contains(['/', '?', '#']) {
            return Err("enter only the host and an optional port, without a path");
        }
        if text.contains('@') {
            return Err("enter only the host and an optional port, without user information");
        }
        let (host, port) = split_port(text)?;
        let port = match port {
            None => DEFAULT_PORT,
            Some(port) => parse_port(port)?,
        };
        let host = if let Some(ip) = host.strip_prefix('[') {
            let ip = ip
                .strip_suffix(']')
                .ok_or("invalid bracketed IPv6 address")?;
            Host::Ip(IpAddr::V6(
                ip.parse().map_err(|_| "invalid bracketed IPv6 address")?,
            ))
        } else if let Ok(ip) = host.parse::<Ipv4Addr>() {
            Host::Ip(IpAddr::V4(ip))
        } else if let Ok(ip) = host.parse::<Ipv6Addr>() {
            Host::Ip(IpAddr::V6(ip))
        } else {
            Host::Name(parse_name(host)?.to_owned())
        };
        if let Host::Ip(ip) = host
            && !is_unicast(ip)
        {
            return Err("a unicast device address is required");
        }
        Ok(Self { host, port })
    }

    /// The device's IP for this output's lifetime: the literal, or the name
    /// resolved within `timeout`, preferring IPv4.
    ///
    /// # Errors
    /// [`Error::Adapter`] when the name does not resolve in time or resolves
    /// only to a non-unicast address.
    pub fn resolve(&self, timeout: Duration) -> Result<IpAddr> {
        let name = match &self.host {
            Host::Ip(ip) => return Ok(*ip),
            Host::Name(name) => name.clone(),
        };
        let port = self.port;
        let lookup = move || {
            (name.as_str(), port)
                .to_socket_addrs()
                .map(Iterator::collect)
        };
        let addresses = resolve_within(lookup, timeout).map_err(|source| {
            Error::adapter(format!("cannot resolve WLED host {}", self.host), source)
        })?;
        let ip = prefer_ipv4(&addresses).ok_or_else(|| {
            Error::adapter(
                format!("cannot resolve WLED host {}", self.host),
                io::Error::new(io::ErrorKind::NotFound, "no addresses"),
            )
        })?;
        if !is_unicast(ip) {
            return Err(Error::other(format!(
                "WLED host {} resolved to {ip}, which is not a unicast address",
                self.host
            )));
        }
        Ok(ip)
    }
}

/// `host[:port]` as configured, for the HTTP `Host` header and messages.
impl fmt::Display for DeviceAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.host)?;
        if self.port != DEFAULT_PORT {
            write!(f, ":{}", self.port)?;
        }
        Ok(())
    }
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ip(IpAddr::V6(ip)) => write!(f, "[{ip}]"),
            Self::Ip(ip) => write!(f, "{ip}"),
            Self::Name(name) => f.write_str(name),
        }
    }
}

/// The first IPv4 address, else the first address: many WLED networks route
/// IPv4 only even when a name also has an IPv6 record.
pub(crate) fn prefer_ipv4(addresses: &[SocketAddr]) -> Option<IpAddr> {
    addresses
        .iter()
        .find(|address| address.is_ipv4())
        .or_else(|| addresses.first())
        .map(SocketAddr::ip)
}

/// Splits off `:port`. A bare IPv6 literal has colons but no port.
fn split_port(text: &str) -> Result<(&str, Option<&str>), &'static str> {
    if text.starts_with('[') {
        let end = text.find(']').ok_or("invalid bracketed IPv6 address")?;
        let (host, rest) = text.split_at(end + 1);
        return match rest {
            "" => Ok((host, None)),
            _ => rest
                .strip_prefix(':')
                .map(|port| (host, Some(port)))
                .ok_or("invalid bracketed IPv6 address"),
        };
    }
    match text.matches(':').count() {
        0 => Ok((text, None)),
        1 => Ok(text
            .split_once(':')
            .map(|(host, port)| (host, Some(port)))
            .unwrap_or((text, None))),
        _ if text.parse::<Ipv6Addr>().is_ok() => Ok((text, None)),
        _ => Err("an IPv6 address with a port needs brackets, such as `[fe80::1]:8080`"),
    }
}

fn parse_port(port: &str) -> Result<u16, &'static str> {
    const REASON: &str = "port must be between 1 and 65535";
    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(REASON);
    }
    port.parse().ok().filter(|&port| port != 0).ok_or(REASON)
}

/// RFC 1123 host name: dot-separated labels of letters, digits and inner hyphens.
fn parse_name(name: &str) -> Result<&str, &'static str> {
    const REASON: &str = "not a valid host name or IP address";
    let valid = name.len() <= 253
        && name.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
        // An all-digit last label is a malformed IPv4 address, not a name.
        && !name
            .rsplit('.')
            .next()
            .is_some_and(|label| label.bytes().all(|byte| byte.is_ascii_digit()));
    valid.then_some(name).ok_or(REASON)
}

fn is_unicast(ip: IpAddr) -> bool {
    let ip = match ip {
        IpAddr::V6(ip) => ip.to_ipv4_mapped().map_or(ip.into(), IpAddr::V4),
        v4 @ IpAddr::V4(_) => v4,
    };
    let broadcast = matches!(ip, IpAddr::V4(ip) if ip.is_broadcast());
    !(ip.is_unspecified() || ip.is_multicast() || broadcast)
}

/// Runs a blocking `lookup` on a helper thread and waits at most `timeout`.
/// The system resolver cannot be cancelled: a lookup that outlives the deadline
/// finishes on its own thread and its result is dropped.
pub(crate) fn resolve_within<F>(lookup: F, timeout: Duration) -> io::Result<Vec<SocketAddr>>
where
    F: FnOnce() -> io::Result<Vec<SocketAddr>> + Send + 'static,
{
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("wled-resolve".into())
        .spawn(move || {
            let _ = sender.send(lookup());
        })?;
    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("no answer within {} ms", timeout.as_millis()),
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(io::Error::other("resolver thread ended without an answer"))
        }
    }
}
