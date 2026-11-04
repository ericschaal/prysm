//! Deterministic lifecycle; only the process adapter executes these actions.
use crate::config::Config;
pub use prysm_webos_protocol::worker::WorkerEvent as Event;
use prysm_webos_service::contract::{Capabilities, SupervisorStatus};
use serde_json::Value;
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Spawn,
    Terminate,
    Kill,
}
// Status flags are independent fields in the existing Luna contract.
#[allow(clippy::struct_excessive_bools)]
pub struct Supervisor {
    pub config: Config,
    pub enabled: bool,
    pub power: Option<String>,
    pub running: bool,
    pub stopping: bool,
    pub error: Option<String>,
    connected: bool,
    busy: bool,
    retry_delay: u64,
    capturing: bool,
    log: Option<String>,
    retry: Option<Instant>,
    kill_at: Option<Instant>,
    video_reset_at: Option<Instant>,
    encoder_at: Option<Instant>,
    immediate: bool,
    expected_exit: bool,
    actions: Vec<Action>,
}
impl Supervisor {
    pub fn new(config: Config) -> Self {
        Self {
            enabled: config.autostart && config.instance_url.is_some(),
            config,
            power: None,
            running: false,
            stopping: false,
            error: None,
            connected: false,
            busy: false,
            retry_delay: 1,
            capturing: false,
            log: None,
            retry: None,
            kill_at: None,
            video_reset_at: None,
            encoder_at: None,
            immediate: false,
            expected_exit: false,
            actions: Vec::new(),
        }
    }
    fn wanted(&self) -> bool {
        self.enabled
            && self.power.as_deref() == Some("Active")
            && self.config.instance_url.is_some()
    }
    pub fn contract(&self) -> SupervisorStatus {
        SupervisorStatus {
            enabled: self.enabled,
            power: self.power.clone(),
            sender_running: self.running,
            receiver_connected: self.connected,
            receiver_busy: self.busy,
            capturing: self.capturing,
            stopping: self.stopping,
            error: self.error.clone(),
            last_log: self.log.clone(),
            config: self.config.clone(),
            capabilities: Capabilities {
                hardware_h264: cfg!(feature = "hardware-h264"),
            },
        }
    }
    pub fn snapshot(&self) -> Value {
        serde_json::to_value(self.contract()).expect("serializable supervisor status")
    }
    pub fn reconcile(&mut self, now: Instant) {
        if !self.wanted() {
            self.stop(now, false);
            return;
        }
        if self.running || self.retry.is_some() {
            return;
        }
        self.running = true;
        self.connected = false;
        self.capturing = false;
        self.error = None;
        self.log = None;
        self.expected_exit = false;
        self.immediate = false;
        self.actions.push(Action::Spawn);
    }
    pub fn set_power(&mut self, power: Option<String>, now: Instant) {
        self.power = power;
        if self.power.as_deref() != Some("Active") {
            self.stop(now, true);
        }
        self.reconcile(now);
    }
    pub fn start(&mut self, now: Instant) -> anyhow::Result<()> {
        self.config.validate()?;
        self.enabled = true;
        self.reconcile(now);
        Ok(())
    }
    pub fn configure(&mut self, config: Config, now: Instant) {
        self.config = config;
        self.busy = false;
        self.retry_delay = 1;
        self.stop(now, true);
        self.reconcile(now);
    }
    pub fn stop(&mut self, now: Instant, immediate: bool) {
        self.retry = None;
        self.immediate |= immediate;
        if self.running && !self.stopping {
            self.stopping = true;
            self.kill_at = Some(now + Duration::from_secs(5));
            self.actions.push(Action::Terminate);
        }
    }
    pub fn tick(&mut self, now: Instant) {
        if self.encoder_at.is_some_and(|deadline| now >= deadline) {
            self.encoder_at = None;
            self.failed("Hardware encoder operation timed out".into(), now);
        }
        if self.video_reset_at.is_some_and(|deadline| now >= deadline) {
            self.video_reset_at = None;
            self.failed("Native video reset timed out".into(), now);
        }
        if self.kill_at.is_some_and(|deadline| now >= deadline) {
            self.kill_at = None;
            self.actions.push(Action::Kill);
        }
        if self.retry.is_some_and(|deadline| now >= deadline) {
            self.retry = None;
        }
        self.reconcile(now);
    }
    pub fn closed(&mut self, success: bool, description: &str, now: Instant) {
        self.running = false;
        self.stopping = false;
        self.connected = false;
        self.capturing = false;
        self.kill_at = None;
        self.video_reset_at = None;
        self.encoder_at = None;
        if self.wanted() && !self.immediate {
            if !success && !self.expected_exit && self.error.is_none() {
                self.error = Some(format!("Capturer exited: {description}"));
            }
            if !success && !self.expected_exit {
                self.retry_delay = self.retry_delay.max(5);
            }
            self.retry = Some(now + Duration::from_secs(self.retry_delay));
            self.retry_delay = (self.retry_delay * 2).min(15);
        }
        self.immediate = false;
        self.reconcile(now);
    }
    pub fn event(&mut self, event: Event, now: Instant) {
        // Ignore late stream data once teardown has begun.
        if !self.running || self.stopping {
            return;
        }
        match event {
            Event::EncoderBusy => self.encoder_at = Some(now + Duration::from_secs(5)),
            Event::EncoderIdle => self.encoder_at = None,
            Event::VideoResetting => self.video_reset_at = Some(now + Duration::from_secs(5)),
            Event::VideoReset => self.video_reset_at = None,
            Event::Connected => {
                self.connected = true;
                self.busy = false;
                self.retry_delay = 1;
            }
            Event::ReceiverBusy => {
                self.busy = true;
                self.connected = false;
                self.expected_exit = true;
            }
            Event::Capturing => self.capturing = true,
            Event::Disconnected => {
                self.busy = false;
                self.expected_exit = true;
                self.connected = false;
                self.capturing = false;
                self.stop(now, false);
            }
            Event::Failed { message } => self.failed(message, now),
        }
    }
    pub fn failed(&mut self, message: String, now: Instant) {
        self.error = Some(message);
        self.connected = false;
        self.capturing = false;
        self.stop(now, false);
    }
    pub fn log(&mut self, value: String) {
        self.log = Some(value);
    }
    pub fn actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn configured() -> Supervisor {
        Supervisor::new(Config {
            instance_url: Some("http://127.0.0.1:8787".parse().unwrap()),
            autostart: true,
            ..Config::default()
        })
    }
    #[test]
    fn snapshot_reports_hardware_build_support() {
        assert_eq!(
            configured().snapshot()["capabilities"]["hardwareH264"],
            cfg!(feature = "hardware-h264")
        );
    }
    #[test]
    fn standby_rapid_wake_and_stop_never_overlap_workers() {
        let now = Instant::now();
        let mut state = configured();
        state.reconcile(now);
        assert_eq!(state.actions(), []);
        state.set_power(Some("Active".into()), now);
        assert_eq!(state.actions(), [Action::Spawn]);
        state.set_power(Some("Suspend".into()), now);
        assert_eq!(state.actions(), [Action::Terminate]);
        state.set_power(Some("Active".into()), now);
        assert_eq!(state.actions(), []);
        state.closed(true, "0", now);
        assert_eq!(state.actions(), [Action::Spawn]);
        state.enabled = false;
        state.stop(now, true);
        assert_eq!(state.actions(), [Action::Terminate]);
        state.tick(now + Duration::from_secs(5));
        assert_eq!(state.actions(), [Action::Kill]);
        state.closed(false, "SIGKILL", now);
        state.tick(now + Duration::from_secs(20));
        assert_eq!(state.actions(), []);
    }
    #[test]
    fn configuration_waits_for_exit_and_manual_stop_survives_wake() {
        let now = Instant::now();
        let mut state = configured();
        state.set_power(Some("Active".into()), now);
        state.actions();
        let mut config = state.config.clone();
        config.instance_url = Some("http://[::1]:8787".parse().unwrap());
        config.autostart = false;
        state.configure(config, now);
        assert_eq!(state.actions(), [Action::Terminate]);
        state.closed(true, "0", now);
        assert_eq!(state.actions(), [Action::Spawn]);
        assert_eq!(
            state.config.instance_url,
            Some("http://[::1]:8787".parse().unwrap())
        );
        assert!(state.enabled);
        state.enabled = false;
        state.stop(now, true);
        state.actions();
        state.closed(true, "0", now);
        state.set_power(Some("Suspend".into()), now);
        state.set_power(Some("Active".into()), now);
        assert_eq!(state.actions(), []);
        state.start(now).unwrap();
        assert_eq!(state.actions(), [Action::Spawn]);
    }
    #[test]
    fn failures_retry_after_five_seconds_and_normal_exit_after_one() {
        for (success, delay) in [(false, 5), (true, 1)] {
            let now = Instant::now();
            let mut state = configured();
            state.set_power(Some("Active".into()), now);
            state.actions();
            state.closed(success, "test exit", now);
            state.tick(now + Duration::from_millis(delay * 1000 - 1));
            assert_eq!(state.actions(), []);
            state.tick(now + Duration::from_secs(delay));
            assert_eq!(state.actions(), [Action::Spawn]);
            state.closed(false, "failed", now);
            state.enabled = false;
            state.stop(now, false);
            state.tick(now + Duration::from_secs(100));
            assert_eq!(state.actions(), []);
        }
    }
    #[test]
    fn handshake_events_define_connectivity_and_disconnect_is_not_a_crash() {
        let now = Instant::now();
        let mut state = configured();
        state.set_power(Some("Active".into()), now);
        state.actions();
        state.log("connected to receiver".into());
        assert_eq!(state.snapshot()["receiverConnected"], false);
        state.event(Event::Connected, now);
        assert_eq!(state.snapshot()["receiverConnected"], true);
        state.event(Event::Capturing, now);
        assert_eq!(state.snapshot()["capturing"], true);
        state.event(Event::Disconnected, now);
        assert_eq!(state.snapshot()["receiverConnected"], false);
        assert_eq!(state.actions(), [Action::Terminate]);
        state.tick(now + Duration::from_secs(5));
        assert_eq!(state.actions(), [Action::Kill]);
        state.closed(false, "SIGKILL", now);
        assert!(state.error.is_none());
        state.event(Event::Connected, now);
        assert_eq!(state.snapshot()["receiverConnected"], false);
        state.tick(now + Duration::from_secs(1));
        assert_eq!(state.actions(), [Action::Spawn]);
    }
}

#[cfg(test)]
mod video_reset_tests {
    use super::*;
    #[test]
    fn completed_reset_keeps_worker_but_stalled_native_release_triggers_recovery() {
        let now = Instant::now();
        let mut state = Supervisor::new(Config {
            instance_url: Some("http://127.0.0.1:8787".parse().unwrap()),
            autostart: true,
            ..Config::default()
        });
        state.set_power(Some("Active".into()), now);
        assert_eq!(state.actions(), [Action::Spawn]);
        state.event(Event::VideoResetting, now);
        state.event(Event::VideoReset, now);
        state.tick(now + Duration::from_secs(6));
        assert_eq!(state.actions(), []);
        state.event(Event::VideoResetting, now + Duration::from_secs(6));
        state.tick(now + Duration::from_secs(11));
        assert_eq!(state.actions(), [Action::Terminate]);
        state.tick(now + Duration::from_secs(16));
        assert_eq!(state.actions(), [Action::Kill]);
    }
}

#[cfg(test)]
mod encoder_tests {
    use super::*;
    #[test]
    fn hardware_start_encode_and_drop_are_watchdog_bounded() {
        let now = Instant::now();
        let mut state = Supervisor::new(Config {
            instance_url: Some("http://127.0.0.1:8787".parse().unwrap()),
            autostart: true,
            ..Config::default()
        });
        state.set_power(Some("Active".into()), now);
        assert_eq!(state.actions(), [Action::Spawn]);
        state.event(Event::EncoderBusy, now);
        state.event(Event::EncoderIdle, now);
        state.tick(now + Duration::from_secs(6));
        assert_eq!(state.actions(), []);
        state.event(Event::EncoderBusy, now + Duration::from_secs(6));
        // A capture event cannot cancel an encoder deadline.
        state.event(Event::VideoReset, now + Duration::from_secs(7));
        state.tick(now + Duration::from_secs(11));
        assert_eq!(state.actions(), [Action::Terminate]);
        assert!(state.error.as_ref().unwrap().contains("encoder"));
        state.tick(now + Duration::from_secs(16));
        assert_eq!(state.actions(), [Action::Kill]);
        state.closed(false, "killed", now + Duration::from_secs(16));
        state.tick(now + Duration::from_secs(21));
        assert_eq!(state.actions(), [Action::Spawn]);
        state.tick(now + Duration::from_secs(27));
        assert_eq!(state.actions(), []);
    }
}
