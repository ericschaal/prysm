//! The service on the real Luna bus.
use crate::identity::Identity;
use crate::install::{FsInstaller, Installer};
use crate::launcher::{Launcher, ProcessLauncher};
use crate::preferences::{FileStore, PreferencesStore};
use crate::service::{Bus, ErrorKind, METHODS, STATE_KEY, Service, WATCHES, Watch, error_reply};
use prysm_webos_luna_sys as sys;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// Register and run until stopped (the signal number stored in `stop`,
/// `deadline`, or uninstall). `wake` wakes the loop: the stop signals and
/// SIGCHLD write to it, and so does the worker's every line.
///
/// # Errors
/// Registration failed or the bus is unavailable (off the TV).
pub fn serve(
    identity: &Identity,
    wake: &sys::Wake,
    stop: Arc<AtomicUsize>,
    deadline: Option<Instant>,
) -> Result<(), sys::Error> {
    let service = Service::new(
        identity.config(),
        ProcessLauncher::new(wake.waker()),
        FileStore::new(identity.preferences()),
        FsInstaller::new(identity.layout.clone()),
        Instant::now(),
    );
    let calls = WATCHES;
    let config = sys::Config {
        name: &identity.service,
        methods: &METHODS,
        calls: &calls,
        wake,
        refuse,
    };
    eprintln!("prysm-webos-service: registering {}", identity.service);
    sys::run(&config, Box::new(Glue::new(service, stop, deadline)))
}

/// The error reply for a call the bus answers itself.
fn refuse(refusal: sys::Refusal, text: &str) -> String {
    let kind = match refusal {
        sys::Refusal::TooLarge => ErrorKind::TooLarge,
        sys::Refusal::Internal => ErrorKind::Internal,
    };
    error_reply(kind, text)
}

/// Adapts the service to the sys crate's handler.
pub(crate) struct Glue<L: Launcher, S: PreferencesStore, I: Installer> {
    service: Service<L, S, I>,
    /// The number of the signal that asked to stop; 0 until one did.
    stop: Arc<AtomicUsize>,
    deadline: Option<Instant>,
    stopping: bool,
}

impl<L: Launcher, S: PreferencesStore, I: Installer> Glue<L, S, I> {
    pub(crate) fn new(
        service: Service<L, S, I>,
        stop: Arc<AtomicUsize>,
        deadline: Option<Instant>,
    ) -> Self {
        Self {
            service,
            stop,
            deadline,
            stopping: false,
        }
    }
}

struct LunaBus<'a>(&'a mut sys::Bus);

impl Bus for LunaBus<'_> {
    fn publish(&mut self, payload: &str) {
        self.0.publish(STATE_KEY, payload);
    }
    fn resubscribe(&mut self, watch: Watch) {
        self.0.recall(watch as usize);
    }
}

impl<L, S, I> sys::Handler for Glue<L, S, I>
where
    L: Launcher,
    S: PreferencesStore,
    I: Installer,
{
    fn request(&mut self, bus: &mut sys::Bus, request: sys::Request<'_>) -> sys::Response {
        let reply = self.service.request(
            &mut LunaBus(bus),
            request.method,
            request.sender,
            request.payload,
            Instant::now(),
        );
        sys::Response {
            payload: reply.payload,
            subscribe: reply.subscribe,
        }
    }

    fn reply(&mut self, bus: &mut sys::Bus, index: usize, payload: &str) {
        let now = Instant::now();
        if index == Watch::Power as usize {
            self.service.power(&mut LunaBus(bus), payload, now);
        } else if index == Watch::Video as usize {
            self.service.video(&mut LunaBus(bus), payload, now);
        }
    }

    fn poll(&mut self, bus: &mut sys::Bus, now: Instant) -> sys::Next {
        if !self.stopping {
            let signal = self.stop.load(Ordering::Relaxed);
            let why = if signal != 0 {
                Some(signal_name(signal))
            } else if self.deadline.is_some_and(|deadline| now >= deadline) {
                Some("the sandboxed run time is over".into())
            } else {
                None
            };
            if let Some(why) = why {
                self.stopping = true;
                eprintln!("prysm-webos-service: stopping: {why}");
                self.service.shutdown(&mut LunaBus(bus), now);
            }
        }
        if !self.service.tick(&mut LunaBus(bus), now) {
            return sys::Next::Stop;
        }
        let deadline = self.deadline.filter(|_| !self.stopping);
        [self.service.next_due(now), deadline]
            .into_iter()
            .flatten()
            .min()
            .map_or(sys::Next::Event, sys::Next::At)
    }
}

fn signal_name(signal: usize) -> String {
    i32::try_from(signal)
        .ok()
        .and_then(|number| nix::sys::signal::Signal::try_from(number).ok())
        .map_or_else(
            || format!("signal {signal}"),
            |signal| signal.as_str().into(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preferences::MemoryStore;
    use crate::service::tests::{FakeInstaller, FakeLauncher, config};
    use std::time::Duration;
    use sys::Handler;

    #[test]
    fn a_stop_signal_shuts_down_and_ends_the_loop() {
        let now = Instant::now();
        let service = Service::new(
            config(),
            FakeLauncher::default(),
            MemoryStore::default(),
            FakeInstaller::default(),
            now,
        );
        let stop = Arc::new(AtomicUsize::new(0));
        let mut glue = Glue::new(service, stop.clone(), None);
        let mut bus = sys::Bus::default();
        // Nothing runs: only the subscriptions' renewal is due.
        assert_eq!(
            glue.poll(&mut bus, now),
            sys::Next::At(now + crate::service::RESUBSCRIBE)
        );
        let reply = glue.request(
            &mut bus,
            sys::Request {
                method: "getState",
                sender: "anyone",
                payload: "{}",
            },
        );
        assert!(reply.payload.contains(r#""returnValue":true"#));
        stop.store(15, Ordering::Relaxed);
        assert_eq!(signal_name(15), "SIGTERM");
        assert_eq!(
            glue.poll(&mut bus, now),
            sys::Next::Stop,
            "no worker: finished at once"
        );
    }

    #[test]
    fn a_probe_deadline_shuts_down() {
        let now = Instant::now();
        let service = Service::new(
            config(),
            FakeLauncher::default(),
            MemoryStore::default(),
            FakeInstaller::default(),
            now,
        );
        let deadline = now + Duration::from_secs(1);
        let mut glue = Glue::new(service, Arc::default(), Some(deadline));
        let mut bus = sys::Bus::default();
        assert_eq!(glue.poll(&mut bus, now), sys::Next::At(deadline));
        assert_eq!(glue.poll(&mut bus, deadline), sys::Next::Stop);
    }

    /// The bus's own refusals carry the service's error kinds.
    #[test]
    fn bus_refusals_are_service_errors() {
        for (refusal, kind) in [
            (sys::Refusal::TooLarge, "too_large"),
            (sys::Refusal::Internal, "internal"),
        ] {
            let reply: serde_json::Value =
                serde_json::from_str(&refuse(refusal, "a \"quoted\"\nline")).unwrap();
            assert_eq!(
                reply,
                serde_json::json!({
                    "returnValue": false,
                    "errorKind": kind,
                    "errorText": "a \"quoted\"\nline",
                })
            );
        }
    }
}
