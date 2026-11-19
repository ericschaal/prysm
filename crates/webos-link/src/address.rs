//! The Prysm a slim worker streams to. Parsing is `http::Uri`'s; only
//! Prysm's own rules are added here.
use http::Uri;
use prysm_ports::CAPTURE_STREAM_PATH;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;

/// How a [`PrysmAddress`] is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    /// Plain HTTP: the portal itself, on the LAN.
    Http,
    /// HTTPS: through a reverse proxy that terminates TLS.
    Https,
}

impl Scheme {
    const fn default_port(self) -> u16 {
        match self {
            Self::Http => 80,
            Self::Https => 443,
        }
    }
}

/// Where a Prysm portal is: `http://host[:port]` or `https://host[:port]`,
/// the host an IP address (IPv6 in brackets) or a name such as
/// `prysm.local`, without a path. A name is resolved at every connection
/// attempt. Serialized as the URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PrysmAddress {
    scheme: Scheme,
    /// An IP address or a host name, without brackets.
    host: String,
    port: u16,
}

impl PrysmAddress {
    #[must_use]
    pub fn scheme(&self) -> Scheme {
        self.scheme
    }

    /// The IP address or host name, without IPv6 brackets.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The port, the scheme's default (80, 443) when the URL has none.
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

    /// The capture stream's WebSocket URL: `ws://` for `http://`, `wss://`
    /// for `https://`, at [`CAPTURE_STREAM_PATH`].
    #[must_use]
    pub fn capture_stream(&self) -> Uri {
        let scheme = match self.scheme {
            Scheme::Http => "ws",
            Scheme::Https => "wss",
        };
        format!("{scheme}://{}{CAPTURE_STREAM_PATH}", self.authority())
            .parse()
            .expect("built from a parsed address")
    }

    /// `host` or `[v6]`, with `:port` unless it is the scheme's default.
    fn authority(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.port == self.scheme.default_port() {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }
}

impl std::fmt::Display for PrysmAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let scheme = match self.scheme {
            Scheme::Http => "http",
            Scheme::Https => "https",
        };
        write!(f, "{scheme}://{}", self.authority())
    }
}

impl FromStr for PrysmAddress {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        const EXAMPLE: &str = "such as http://192.168.1.20:8787 or https://prysm.example.com";
        let uri: Uri = text
            .parse()
            .map_err(|_| format!("is not an address, {EXAMPLE}"))?;
        let scheme = match uri.scheme_str() {
            Some("http") => Scheme::Http,
            Some("https") => Scheme::Https,
            _ => return Err(format!("needs http:// or https://, {EXAMPLE}")),
        };
        let authority = uri
            .authority()
            .ok_or_else(|| format!("needs a host, {EXAMPLE}"))?;
        let extra = uri
            .path_and_query()
            .is_some_and(|rest| !matches!(rest.as_str(), "" | "/"));
        if extra || authority.as_str().contains('@') {
            return Err(format!("needs only the address, without a path, {EXAMPLE}"));
        }
        let port = match authority.port_u16() {
            Some(0) => return Err(format!("needs a port other than 0, {EXAMPLE}")),
            Some(port) => port,
            None => scheme.default_port(),
        };
        let host = authority.host();
        let host = host
            .strip_prefix('[')
            .and_then(|v6| v6.strip_suffix(']'))
            .unwrap_or(host);
        if host.parse::<IpAddr>().is_ok_and(|ip| ip.is_unspecified()) {
            return Err(format!("needs Prysm's address, not {host}"));
        }
        Ok(Self {
            scheme,
            host: host.to_owned(),
            port,
        })
    }
}

impl TryFrom<String> for PrysmAddress {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        text.parse()
            .map_err(|reason| format!("Prysm address `{text}`: {reason}"))
    }
}

impl From<PrysmAddress> for String {
    fn from(address: PrysmAddress) -> Self {
        address.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_ports_and_the_stream_url() {
        for (text, port, shown, stream) in [
            (
                "http://192.168.1.20:8787/",
                8787,
                "http://192.168.1.20:8787",
                "ws://192.168.1.20:8787/api/v1/capture/stream",
            ),
            (
                "http://prysm.local",
                80,
                "http://prysm.local",
                "ws://prysm.local/api/v1/capture/stream",
            ),
            (
                "https://prysm.example.com",
                443,
                "https://prysm.example.com",
                "wss://prysm.example.com/api/v1/capture/stream",
            ),
            (
                "https://[fd00::20]:8443",
                8443,
                "https://[fd00::20]:8443",
                "wss://[fd00::20]:8443/api/v1/capture/stream",
            ),
        ] {
            let address: PrysmAddress = text.parse().unwrap();
            assert_eq!(address.port(), port, "{text}");
            assert_eq!(address.to_string(), shown);
            assert_eq!(address.capture_stream().to_string(), stream);
        }
        let v6: PrysmAddress = "http://[fd00::20]:8787".parse().unwrap();
        assert_eq!(v6.host(), "fd00::20");
        assert_eq!(v6.socket_addr(), Some("[fd00::20]:8787".parse().unwrap()));
        let named: PrysmAddress = "http://prysm.local:8787".parse().unwrap();
        assert_eq!(named.socket_addr(), None);
    }

    #[test]
    fn only_an_http_or_https_address() {
        for bad in [
            "192.168.1.20:8787",
            "ws://prysm.local",
            "http://prysm.local:0",
            "http://0.0.0.0:8787",
            "http://[::]:8787",
            "http://prysm.local/api",
            "http://prysm.local/?x",
            "http://user@prysm.local",
        ] {
            assert!(bad.parse::<PrysmAddress>().is_err(), "{bad}");
        }
    }

    #[test]
    fn serializes_as_the_url() {
        let address: PrysmAddress = serde_json::from_str(r#""https://prysm.example.com""#).unwrap();
        assert_eq!(
            serde_json::to_string(&address).unwrap(),
            r#""https://prysm.example.com""#
        );
        let error = serde_json::from_str::<PrysmAddress>(r#""prysm.local:7380""#).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("Prysm address `prysm.local:7380`: ")
        );
    }
}
