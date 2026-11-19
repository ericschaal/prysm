//! Host name lookup with a deadline, for adapters that resolve a configured
//! name each time they connect.
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

/// A blocking lookup of `host:port`.
pub type Lookup = fn(&str, u16) -> io::Result<SocketAddr>;

/// The system resolver (`getaddrinfo`: hosts file, DNS, and mDNS where the
/// system has it), which blocks without a deadline of its own. The first IPv4
/// address, else the first address: many networks route IPv4 only even when
/// a name also has an IPv6 record.
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

/// Resolves one `host:port`, waiting at most a given time per call.
///
/// The lookup runs on a helper thread because the system resolver cannot be
/// cancelled. A lookup still running when the wait ends is kept, never
/// repeated: a later [`resolve`](Self::resolve) takes its answer. So at most
/// one lookup thread exists per resolver, and a slow resolver still gets
/// through. Dropping the resolver never waits for that thread.
#[derive(Debug)]
pub struct Resolver {
    host: String,
    port: u16,
    lookup: Lookup,
    pending: Option<mpsc::Receiver<io::Result<SocketAddr>>>,
}

impl Resolver {
    /// A resolver using [`system_lookup`].
    #[must_use]
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            lookup: system_lookup,
            pending: None,
        }
    }

    /// Replaces the system resolver, for tests.
    #[must_use]
    pub fn with_lookup(mut self, lookup: Lookup) -> Self {
        self.lookup = lookup;
        self
    }

    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Looks the name up anew, or takes the answer of the lookup still
    /// pending from an earlier call, waiting at most `wait`.
    ///
    /// # Errors
    /// The lookup failed, gave no answer within `wait`
    /// ([`io::ErrorKind::TimedOut`]; it stays pending), or its thread could
    /// not be spawned or panicked.
    pub fn resolve(&mut self, wait: Duration) -> io::Result<SocketAddr> {
        let pending = match self.pending.take() {
            Some(pending) => pending,
            None => self.start()?,
        };
        match pending.recv_timeout(wait) {
            Ok(found) => found,
            Err(RecvTimeoutError::Timeout) => {
                self.pending = Some(pending);
                Err(io::Error::new(io::ErrorKind::TimedOut, "no answer yet"))
            }
            Err(RecvTimeoutError::Disconnected) => {
                Err(io::Error::other("the lookup thread panicked"))
            }
        }
    }

    fn start(&self) -> io::Result<mpsc::Receiver<io::Result<SocketAddr>>> {
        let (answer, pending) = mpsc::sync_channel(1);
        let (host, port, lookup) = (self.host.clone(), self.port, self.lookup);
        thread::Builder::new()
            .name("prysm-resolve".into())
            .spawn(move || {
                // Nobody waits any more once the resolver is dropped; that is fine.
                let _ = answer.send(lookup(&host, port));
            })?;
        Ok(pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Instant;

    const WAIT: Duration = Duration::from_secs(5);

    #[test]
    fn a_name_is_looked_up_at_every_call() {
        static CALLS: AtomicU32 = AtomicU32::new(0);
        let mut named = Resolver::new("prysm.local", 7380).with_lookup(|host, port| {
            assert_eq!((host, port), ("prysm.local", 7380));
            // The address changes between calls (a new DHCP lease).
            let n = CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(SocketAddr::from(([192, 168, 1, 20 + n as u8], port)))
        });
        assert_eq!(
            named.resolve(WAIT).unwrap(),
            "192.168.1.20:7380".parse().unwrap()
        );
        assert_eq!(
            named.resolve(WAIT).unwrap(),
            "192.168.1.21:7380".parse().unwrap()
        );
    }

    #[test]
    fn a_failed_lookup_keeps_its_cause() {
        let mut missing = Resolver::new("nowhere.example", 80)
            .with_lookup(|_, _| Err(io::Error::new(io::ErrorKind::NotFound, "no such host")));
        let error = missing.resolve(WAIT).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(error.to_string(), "no such host");
    }

    /// A slow lookup delays a call by at most the wait; its answer is taken
    /// by a later call, and only one lookup runs at a time.
    #[test]
    fn a_slow_lookup_is_bounded_and_answers_a_later_call() {
        static CALLS: AtomicU32 = AtomicU32::new(0);
        let mut slow = Resolver::new("prysm.local", 7380).with_lookup(|_, port| {
            CALLS.fetch_add(1, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(300));
            Ok(SocketAddr::from(([10, 0, 0, 2], port)))
        });
        let started = Instant::now();
        let error = slow.resolve(Duration::from_millis(20)).unwrap_err();
        assert!(started.elapsed() < Duration::from_millis(250), "bounded");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(
            slow.resolve(WAIT).unwrap(),
            "10.0.0.2:7380".parse().unwrap()
        );
        assert_eq!(CALLS.load(Ordering::SeqCst), 1, "one lookup at a time");
    }

    #[test]
    fn the_system_resolver_prefers_ipv4() {
        let address = system_lookup("localhost", 7380).unwrap();
        assert_eq!(address, "127.0.0.1:7380".parse().unwrap());
    }
}
