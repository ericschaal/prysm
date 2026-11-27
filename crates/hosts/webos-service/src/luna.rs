//! The service on the real Luna bus.
use crate::identity::Identity;
use crate::install::{FsInstaller, Installer};
use crate::launcher::{Launcher, ProcessLauncher};
use crate::preferences::{FileStore, PreferencesStore};
use crate::service::{
    Bus, ErrorKind, ErrorReply, METHODS, STATE_KEY, Service, WATCHES, Watch, encode_reply,
};
use crate::tail::Tail;
use prysm_webos_luna_sys as sys;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use tracing::info;

/// Register and run until stopped (the signal number stored in `stop`,
/// `deadline`, or uninstall). `wake` wakes the loop: the stop signals and
/// SIGCHLD write to it, and so do the worker's every line and the
/// supervisor's every log event, which `tail` queues from now on.
///
/// # Errors
/// Registration failed or the bus is unavailable (off the TV).
pub fn serve(
    identity: &Identity,
    wake: &sys::Wake,
    tail: &Tail,
    stop: Arc<AtomicUsize>,
    deadline: Option<Instant>,
) -> Result<(), sys::Error> {
    tail.attach(wake.waker());
    let service = Service::new(
        identity.config(),
        ProcessLauncher::new(wake.waker()),
        FileStore::new(identity.preferences()),
        FsInstaller::new(identity.layout.clone()),
        Instant::now(),
    );
    let config = sys::Config {
        name: &identity.service,
        methods: &METHODS,
        calls: &WATCHES,
        wake,
        refuse,
    };
    info!(service = %identity.service, "registering");
    sys::run(
        &config,
        Box::new(Glue::new(service, tail.clone(), stop, deadline)),
    )
}

/// The error reply for a call the bus answers itself.
fn refuse(refusal: sys::Refusal, text: &str) -> String {
    let kind = match refusal {
        sys::Refusal::TooLarge => ErrorKind::TooLarge,
        sys::Refusal::Internal => ErrorKind::Internal,
    };
    encode_reply(&ErrorReply::new(kind, text))
}

/// Adapts the service to the sys crate's handler.
pub(crate) struct Glue<L: Launcher, S: PreferencesStore, I: Installer> {
    service: Service<L, S, I>,
    /// The supervisor's log lines, for the state's tail.
    tail: Tail,
    /// The number of the signal that asked to stop; 0 until one did.
    stop: Arc<AtomicUsize>,
    deadline: Option<Instant>,
    stopping: bool,
}

impl<L: Launcher, S: PreferencesStore, I: Installer> Glue<L, S, I> {
    pub(crate) fn new(
        service: Service<L, S, I>,
        tail: Tail,
        stop: Arc<AtomicUsize>,
        deadline: Option<Instant>,
    ) -> Self {
        Self {
            service,
            tail,
            stop,
            deadline,
            stopping: false,
        }
    }
}

impl Bus for sys::Bus {
    fn publish(&mut self, payload: &str) {
        self.publish(STATE_KEY, payload);
    }
    fn resubscribe(&mut self, watch: Watch) {
        self.recall(watch as usize);
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
            bus,
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
            self.service.power(bus, payload, now);
        } else if index == Watch::Video as usize {
            self.service.video(bus, payload, now);
        }
    }

    fn poll(&mut self, bus: &mut sys::Bus, now: Instant) -> sys::Next {
        // What the supervisor logged since the last poll goes before the
        // worker lines `tick` reads: in the order they came when a poll lies
        // between them, the supervisor's first when not. Anything logged
        // while these are handled wakes the loop for the next poll.
        for line in self.tail.drain() {
            self.service.log(bus, line, now);
        }
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
                info!(%why, "stopping");
                self.service.shutdown(bus, now);
            }
        }
        if !self.service.tick(bus, now) {
            return sys::Next::Stop;
        }
        let deadline = self.deadline.filter(|_| !self.stopping);
        [self.service.next_due(), deadline]
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
    use crate::launcher::Output;
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
        let mut glue = Glue::new(service, Tail::default(), stop.clone(), None);
        let mut bus = sys::Bus::default();
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
        let mut glue = Glue::new(service, Tail::default(), Arc::default(), Some(deadline));
        let mut bus = sys::Bus::default();
        assert_eq!(glue.poll(&mut bus, now), sys::Next::At(deadline));
        assert_eq!(glue.poll(&mut bus, deadline), sys::Next::Stop);
    }

    /// The supervisor's log lines reach the tail at the next poll, in order
    /// with the worker's: logged before a poll, and while a poll runs (they
    /// wait for the next one).
    #[test]
    fn the_supervisors_log_lines_join_the_workers_in_the_tail() {
        let now = Instant::now();
        let launcher = FakeLauncher::default();
        let world = launcher.0.clone();
        let wake = sys::Wake::new().unwrap();
        let tail = Tail::default();
        tail.attach(wake.waker());
        let writer = {
            let tail = tail.clone();
            move || tail.writer()
        };
        let subscriber = prysm_logging::default_subscriber(crate::identity::SUPERVISOR, writer);
        let log = tracing::subscriber::with_default(subscriber, || {
            let service = Service::new(
                config(),
                launcher,
                MemoryStore::default(),
                FakeInstaller::default(),
                now,
            );
            let mut glue = Glue::new(service, tail.clone(), Arc::default(), None);
            let mut bus = sys::Bus::default();
            let worker = |line: &str| {
                let line = Output::Log(line.into());
                world.borrow_mut().outputs.push_back(line);
            };
            let active = r#"{"returnValue":true,"state":"Active"}"#;
            glue.reply(&mut bus, Watch::Power as usize, active);
            info!("before the first poll");
            worker("prysm-webos: first");
            glue.poll(&mut bus, now);
            world.borrow_mut().wait_fails = true;
            worker("prysm-webos: second");
            glue.poll(&mut bus, now);
            glue.poll(&mut bus, now);
            let state = glue.request(
                &mut bus,
                sys::Request {
                    method: "getState",
                    sender: "anyone",
                    payload: "{}",
                },
            );
            let state: serde_json::Value = serde_json::from_str(&state.payload).unwrap();
            state["log"].clone()
        });
        assert_eq!(
            log,
            serde_json::json!([
                "prysm-webos-service: info prysm_webos_service::luna::tests: before the first poll",
                "prysm-webos: first",
                "prysm-webos: second",
                "prysm-webos-service: warn prysm_webos_service::service: cannot wait for the worker error=no child",
            ])
        );
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
