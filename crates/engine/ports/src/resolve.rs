//! Host name lookup with a deadline, for adapters that resolve a configured
//! name each time they connect.
use crate::{CallError, Hangup, Stopping};
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::panic::catch_unwind;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

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
    (host, port)
        .to_socket_addrs()?
        .min_by_key(SocketAddr::is_ipv6)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no address"))
}

/// Resolves one `host:port`, waiting at most a given time per call.
///
/// The lookup runs on a helper thread because the system resolver cannot be
/// cancelled. A lookup still running when the wait ends (its time, or a cut
/// of its [`Hangup`]) is kept, never repeated: a later
/// [`resolve`](Self::resolve) takes its answer. So at most one lookup thread
/// exists per resolver, and a slow resolver still gets through. Dropping the
/// resolver never waits for that thread.
#[derive(Debug)]
pub struct Resolver {
    host: String,
    port: u16,
    lookup: Lookup,
    pending: Option<Pending>,
}

/// A lookup under way: its answer, and a cut of a wait for it, come on one
/// channel.
#[derive(Debug)]
struct Pending {
    wake: mpsc::Sender<Looked>,
    answers: mpsc::Receiver<Looked>,
}

#[derive(Debug)]
enum Looked {
    Found(io::Result<SocketAddr>),
    /// A [`Hangup`] was cut while a wait was registered with it.
    Cut,
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
    /// pending from an earlier call, waiting at most `wait`, and ending at
    /// once when `hangup` is cut.
    ///
    /// # Errors
    /// [`CallError::Stopped`] once `hangup` is cut (the lookup stays
    /// pending); else the lookup failed, gave no answer within `wait`
    /// ([`io::ErrorKind::TimedOut`]; it stays pending), or its thread could
    /// not be spawned or panicked.
    pub fn resolve(&mut self, wait: Duration, hangup: &Hangup) -> Result<SocketAddr, CallError> {
        let pending = match self.pending.take() {
            Some(pending) => pending,
            None => self.start()?,
        };
        let deadline = Instant::now().checked_add(wait);
        let wake = pending.wake.clone();
        let registered = hangup.on_cut(move || {
            let _ = wake.send(Looked::Cut);
        });
        let ended = match registered {
            None => Err(Stopping.into()),
            Some(_registration) => loop {
                hangup.waiting();
                let left = deadline.map_or(wait, |at| at.saturating_duration_since(Instant::now()));
                match pending.answers.recv_timeout(left) {
                    Ok(Looked::Found(found)) => return Ok(found?),
                    // The cut of an earlier wait, which ended before it came.
                    Ok(Looked::Cut) if !hangup.is_cut() => {}
                    Ok(Looked::Cut) => break Err(Stopping.into()),
                    Err(RecvTimeoutError::Timeout) => {
                        break Err(io::Error::new(io::ErrorKind::TimedOut, "no answer yet").into());
                    }
                    // This end holds a sender: the channel never closes.
                    Err(RecvTimeoutError::Disconnected) => {
                        break Err(io::Error::other("the lookup channel closed").into());
                    }
                }
            },
        };
        self.pending = Some(pending);
        ended
    }

    fn start(&self) -> io::Result<Pending> {
        let (wake, answers) = mpsc::channel();
        let answer = wake.clone();
        let (host, port, lookup) = (self.host.clone(), self.port, self.lookup);
        thread::Builder::new()
            .name("prysm-resolve".into())
            .spawn(move || {
                let found = catch_unwind(|| lookup(&host, port))
                    .unwrap_or_else(|_| Err(io::Error::other("the lookup thread panicked")));
                // Nobody waits any more once the resolver is dropped; that is fine.
                let _ = answer.send(Looked::Found(found));
            })?;
        Ok(Pending { wake, answers })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Condvar, Mutex};

    const WAIT: Duration = Duration::from_secs(5);

    #[test]
    fn a_name_is_looked_up_at_every_call() {
        static CALLS: AtomicU32 = AtomicU32::new(0);
        let mut named = Resolver::new("prysm.local", 7380).with_lookup(|host, port| {
            assert_eq!((host, port), ("prysm.local", 7380));
            let n = CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(SocketAddr::from(([192, 168, 1, 20 + n as u8], port)))
        });
        assert_eq!(
            named.resolve(WAIT, &Hangup::default()).unwrap(),
            "192.168.1.20:7380".parse().unwrap()
        );
        assert_eq!(
            named.resolve(WAIT, &Hangup::default()).unwrap(),
            "192.168.1.21:7380".parse().unwrap()
        );
    }

    #[test]
    fn a_failed_lookup_keeps_its_cause() {
        let mut missing = Resolver::new("nowhere.example", 80)
            .with_lookup(|_, _| Err(io::Error::new(io::ErrorKind::NotFound, "no such host")));
        let CallError::Failed(error) = missing.resolve(WAIT, &Hangup::default()).unwrap_err()
        else {
            panic!("nothing cut it");
        };
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
        let error = slow
            .resolve(Duration::from_millis(20), &Hangup::default())
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_millis(250), "bounded");
        assert!(
            matches!(&error, CallError::Failed(error) if error.kind() == io::ErrorKind::TimedOut),
            "{error}"
        );
        assert_eq!(
            slow.resolve(WAIT, &Hangup::default()).unwrap(),
            "10.0.0.2:7380".parse().unwrap()
        );
        assert_eq!(CALLS.load(Ordering::SeqCst), 1, "one lookup at a time");
    }

    /// A cut ends a wait for a lookup that has not answered, and every wait
    /// after it; the lookup is kept, and a wait without a cut takes its
    /// answer once it comes.
    #[test]
    fn a_cut_ends_the_wait_and_keeps_the_lookup() {
        static CALLS: AtomicU32 = AtomicU32::new(0);
        static GATE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
        let mut held = Resolver::new("prysm.local", 7380).with_lookup(|_, port| {
            CALLS.fetch_add(1, Ordering::SeqCst);
            let (open, opened) = &GATE;
            let _open = opened
                .wait_while(open.lock().unwrap(), |open| !*open)
                .unwrap();
            Ok(SocketAddr::from(([10, 0, 0, 2], port)))
        });
        let hangup = Hangup::default();
        let cutting = {
            let hangup = hangup.clone();
            thread::spawn(move || hangup.cut())
        };
        let error = held.resolve(Duration::from_secs(60), &hangup).unwrap_err();
        assert!(matches!(error, CallError::Stopped(Stopping)), "{error}");
        cutting.join().unwrap();
        let again = held.resolve(Duration::from_secs(60), &hangup).unwrap_err();
        assert!(matches!(again, CallError::Stopped(Stopping)), "{again}");
        *GATE.0.lock().unwrap() = true;
        GATE.1.notify_all();
        assert_eq!(
            held.resolve(WAIT, &Hangup::default()).unwrap(),
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
