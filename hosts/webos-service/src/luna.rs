//! The service on the real Luna bus.
use crate::identity::Identity;
use crate::install::{FsInstaller, Installer};
use crate::launcher::{Launcher, ProcessLauncher};
use crate::preferences::{FileStore, PreferencesStore};
use crate::service::{Bus, METHODS, STATE_KEY, Service, WATCHES, Watch};
use prysm_webos_luna_sys as sys;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(100);

/// Register and run until stopped (signal, `deadline`, or uninstall).
///
/// # Errors
/// Registration failed or the bus is unavailable (off the TV).
pub fn serve(
    identity: &Identity,
    stop: Arc<AtomicBool>,
    deadline: Option<Instant>,
) -> Result<(), sys::Error> {
    let service = Service::new(
        identity.config(),
        ProcessLauncher,
        FileStore::new(identity.preferences()),
        FsInstaller::new(identity.layout.clone()),
        Instant::now(),
    );
    let calls = WATCHES;
    let config = sys::Config {
        name: &identity.service,
        methods: &METHODS,
        calls: &calls,
        tick: TICK,
    };
    eprintln!("prysm-webos-service: registering {}", identity.service);
    sys::run(&config, Box::new(Glue::new(service, stop, deadline)))
}

/// Adapts the service to the sys crate's handler.
pub(crate) struct Glue<L: Launcher, S: PreferencesStore, I: Installer> {
    service: Service<L, S, I>,
    stop: Arc<AtomicBool>,
    deadline: Option<Instant>,
    stopping: bool,
}

impl<L: Launcher, S: PreferencesStore, I: Installer> Glue<L, S, I> {
    pub(crate) fn new(
        service: Service<L, S, I>,
        stop: Arc<AtomicBool>,
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

    fn tick(&mut self, bus: &mut sys::Bus, now: Instant) -> bool {
        let expired = self.deadline.is_some_and(|deadline| now >= deadline);
        if !self.stopping && (expired || self.stop.load(Ordering::Relaxed)) {
            self.stopping = true;
            eprintln!("prysm-webos-service: stopping");
            self.service.shutdown(&mut LunaBus(bus), now);
        }
        self.service.tick(&mut LunaBus(bus), now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preferences::MemoryStore;
    use crate::service::tests::{FakeInstaller, FakeLauncher, config};
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
        let stop = Arc::new(AtomicBool::new(false));
        let mut glue = Glue::new(service, stop.clone(), None);
        let mut bus = sys::Bus::default();
        assert!(glue.tick(&mut bus, now));
        let reply = glue.request(
            &mut bus,
            sys::Request {
                method: "getState",
                sender: "anyone",
                payload: "{}",
            },
        );
        assert!(reply.payload.contains(r#""returnValue":true"#));
        stop.store(true, Ordering::Relaxed);
        assert!(!glue.tick(&mut bus, now), "no worker: finished at once");
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
        let mut glue = Glue::new(service, Arc::default(), Some(now + Duration::from_secs(1)));
        let mut bus = sys::Bus::default();
        assert!(glue.tick(&mut bus, now));
        assert!(!glue.tick(&mut bus, now + Duration::from_secs(1)));
    }
}
