//! The receiver's address, resolved at every connection attempt.
use prysm_ports::{Error, Result};
use prysm_webos_link::ReceiverAddress;
use std::net::SocketAddr;
use std::time::Duration;

/// How long one connection attempt waits for a host name to resolve.
pub const RESOLVE_WAIT: Duration = Duration::from_millis(500);

/// Turns the receiver's `host:port` into the address to connect to. An IP
/// address is used as is. A name is looked up anew at each call, so a
/// changed address is picked up by the next reconnect, with
/// [`prysm_ports::Resolver`]: the caller (the forwarder thread, between two
/// pulls) waits at most [`RESOLVE_WAIT`], and a lookup still running then
/// answers a later call. Shutdown never waits for it.
pub struct Resolver {
    receiver: ReceiverAddress,
    names: prysm_ports::Resolver,
    wait: Duration,
}

impl Resolver {
    #[must_use]
    pub fn new(receiver: ReceiverAddress) -> Self {
        Self {
            names: prysm_ports::Resolver::new(receiver.host(), receiver.port()),
            receiver,
            wait: RESOLVE_WAIT,
        }
    }

    #[cfg(test)]
    pub fn with_lookup(mut self, lookup: prysm_ports::Lookup, wait: Duration) -> Self {
        self.names = self.names.with_lookup(lookup);
        self.wait = wait;
        self
    }

    /// # Errors
    /// The name does not resolve, or no answer came within [`RESOLVE_WAIT`].
    pub fn resolve(&mut self) -> Result<SocketAddr> {
        if let Some(address) = self.receiver.socket_addr() {
            return Ok(address);
        }
        self.names
            .resolve(self.wait)
            .map_err(|error| Error::adapter(format!("cannot resolve {}", self.receiver), error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    fn resolver(text: &str, lookup: prysm_ports::Lookup, wait: Duration) -> Resolver {
        Resolver::new(text.parse().unwrap()).with_lookup(lookup, wait)
    }

    #[test]
    fn an_ip_address_is_used_without_a_lookup() {
        let mut ip = resolver(
            "192.168.1.20:7380",
            |_, _| panic!("no lookup for an IP address"),
            RESOLVE_WAIT,
        );
        assert_eq!(ip.resolve().unwrap(), "192.168.1.20:7380".parse().unwrap());
        let mut v6 = resolver("[fe80::1]:7380", |_, _| panic!(), RESOLVE_WAIT);
        assert_eq!(v6.resolve().unwrap(), "[fe80::1]:7380".parse().unwrap());
    }

    #[test]
    fn a_name_is_looked_up() {
        let mut named = resolver(
            "prysm.local:7380",
            |host, port| {
                assert_eq!((host, port), ("prysm.local", 7380));
                Ok(SocketAddr::from(([192, 168, 1, 20], port)))
            },
            RESOLVE_WAIT,
        );
        assert_eq!(
            named.resolve().unwrap(),
            "192.168.1.20:7380".parse().unwrap()
        );
    }

    #[test]
    fn a_name_that_does_not_resolve_names_itself() {
        let mut missing = resolver(
            "nowhere.example:7380",
            |_, _| Err(io::Error::new(io::ErrorKind::NotFound, "no such host")),
            RESOLVE_WAIT,
        );
        let error = prysm_ports::chain(&missing.resolve().unwrap_err());
        assert_eq!(error, "cannot resolve nowhere.example:7380: no such host");
    }

    /// A slow resolver delays an attempt by at most the wait, and says so.
    #[test]
    fn a_slow_lookup_is_a_bounded_wait() {
        let mut slow = resolver(
            "prysm.local:7380",
            |_, port| {
                std::thread::sleep(Duration::from_millis(300));
                Ok(SocketAddr::from(([10, 0, 0, 2], port)))
            },
            Duration::from_millis(20),
        );
        let started = std::time::Instant::now();
        let error = prysm_ports::chain(&slow.resolve().unwrap_err());
        assert!(started.elapsed() < Duration::from_millis(250), "bounded");
        assert_eq!(error, "cannot resolve prysm.local:7380: no answer yet");
    }
}
