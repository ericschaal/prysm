//! The receiver's address, resolved at every connection attempt.
use prysm_ports::{Error, Result};
use prysm_webos_link::ReceiverAddress;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

/// How long one connection attempt waits for a host name to resolve.
pub const RESOLVE_WAIT: Duration = Duration::from_millis(500);

/// A blocking lookup of `host:port`.
pub type Lookup = fn(&str, u16) -> io::Result<SocketAddr>;

/// The system resolver (`getaddrinfo`: hosts file, DNS, and mDNS where the
/// TV has it), which blocks without a deadline of its own. The first IPv4
/// address, else the first address: receivers usually listen on
/// `0.0.0.0`, and a name often resolves to an IPv6 address first.
///
/// # Errors
/// The name does not resolve, or resolves to no address.
pub fn system_lookup(host: &str, port: u16) -> io::Result<SocketAddr> {
    let addresses: Vec<SocketAddr> = (host, port).to_socket_addrs()?.collect();
    addresses
        .iter()
        .find(|address| address.is_ipv4())
        .or_else(|| addresses.first())
        .copied()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no address"))
}

/// Turns the receiver's `host:port` into the address to connect to. An IP
/// address is used as is. A name is looked up anew at each call, so a
/// changed address is picked up by the next reconnect; the lookup runs on a
/// helper thread because std's resolver blocks without a deadline, and the
/// caller (the forwarder thread, between two pulls) waits at most
/// [`RESOLVE_WAIT`]. A lookup still running then is kept, never repeated:
/// a later call takes its answer, so at most one lookup thread exists and a
/// slow resolver still gets through. Shutdown never waits for it.
pub struct Resolver {
    receiver: ReceiverAddress,
    lookup: Lookup,
    wait: Duration,
    pending: Option<mpsc::Receiver<io::Result<SocketAddr>>>,
}

impl Resolver {
    #[must_use]
    pub fn new(receiver: ReceiverAddress) -> Self {
        Self {
            receiver,
            lookup: system_lookup,
            wait: RESOLVE_WAIT,
            pending: None,
        }
    }

    #[cfg(test)]
    pub fn with_lookup(mut self, lookup: Lookup, wait: Duration) -> Self {
        self.lookup = lookup;
        self.wait = wait;
        self
    }

    /// # Errors
    /// The name does not resolve, or no answer came within [`RESOLVE_WAIT`].
    pub fn resolve(&mut self) -> Result<SocketAddr> {
        if let Some(address) = self.receiver.socket_addr() {
            return Ok(address);
        }
        let failed = |error| Error::adapter(format!("cannot resolve {}", self.receiver), error);
        let pending = match self.pending.take() {
            Some(pending) => pending,
            None => self.start().map_err(failed)?,
        };
        match pending.recv_timeout(self.wait) {
            Ok(found) => found.map_err(failed),
            Err(RecvTimeoutError::Timeout) => {
                self.pending = Some(pending);
                Err(failed(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "no answer yet",
                )))
            }
            Err(RecvTimeoutError::Disconnected) => Err(Error::Panicked("receiver lookup")),
        }
    }

    fn start(&self) -> io::Result<mpsc::Receiver<io::Result<SocketAddr>>> {
        let (answer, pending) = mpsc::sync_channel(1);
        let (host, port, lookup) = (
            self.receiver.host().to_owned(),
            self.receiver.port(),
            self.lookup,
        );
        std::thread::Builder::new()
            .name("prysm-resolve".into())
            .spawn(move || {
                // Nobody is waiting any more after shutdown; that is fine.
                let _ = answer.send(lookup(&host, port));
            })?;
        Ok(pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn resolver(text: &str, lookup: Lookup, wait: Duration) -> Resolver {
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
    fn a_name_is_looked_up_at_every_attempt() {
        static CALLS: AtomicU32 = AtomicU32::new(0);
        let mut named = resolver(
            "prysm.local:7380",
            |host, port| {
                assert_eq!((host, port), ("prysm.local", 7380));
                // The address changes between attempts (a new DHCP lease).
                let n = CALLS.fetch_add(1, Ordering::SeqCst);
                Ok(SocketAddr::from(([192, 168, 1, 20 + n as u8], port)))
            },
            RESOLVE_WAIT,
        );
        assert_eq!(
            named.resolve().unwrap(),
            "192.168.1.20:7380".parse().unwrap()
        );
        assert_eq!(
            named.resolve().unwrap(),
            "192.168.1.21:7380".parse().unwrap()
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

    /// A slow resolver delays an attempt by at most the wait; its answer is
    /// taken by a later attempt, and only one lookup runs at a time.
    #[test]
    fn a_slow_lookup_is_bounded_and_answers_a_later_attempt() {
        static CALLS: AtomicU32 = AtomicU32::new(0);
        let mut slow = resolver(
            "prysm.local:7380",
            |_, port| {
                CALLS.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(300));
                Ok(SocketAddr::from(([10, 0, 0, 2], port)))
            },
            Duration::from_millis(20),
        );
        let started = std::time::Instant::now();
        let error = prysm_ports::chain(&slow.resolve().unwrap_err());
        assert!(started.elapsed() < Duration::from_millis(250), "bounded");
        assert_eq!(error, "cannot resolve prysm.local:7380: no answer yet");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let address = loop {
            assert!(std::time::Instant::now() < deadline, "never answered");
            if let Ok(address) = slow.resolve() {
                break address;
            }
        };
        assert_eq!(address, "10.0.0.2:7380".parse().unwrap());
        assert_eq!(CALLS.load(Ordering::SeqCst), 1, "one lookup at a time");
    }

    #[test]
    fn the_system_resolver_prefers_ipv4() {
        let address = system_lookup("localhost", 7380).unwrap();
        assert_eq!(address, "127.0.0.1:7380".parse().unwrap());
    }
}
