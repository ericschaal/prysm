//! Deterministic lifecycle; only the process adapter executes these actions.
use crate::config::{CaptureTarget, Config, SetupError};
pub use prysm_webos_protocol::worker::WorkerEvent as Event;
use prysm_webos_service::contract::{Capabilities, SupervisorStatus};
use serde_json::Value;
use std::time::{Duration, Instant};

// Leave room for the handshake and first frame inside the host's 12-second startup window.
const MAX_RETRY_SECONDS: u64 = 5;

/// Only a validated selection can be committed and scheduled for launch.
pub struct Selection {
    preferences: Config,
    target: CaptureTarget,
}
impl TryFrom<Config> for Selection {
    type Error = SetupError;
    fn try_from(preferences: Config) -> Result<Self, Self::Error> {
        Ok(Self {
            target: preferences.resolve()?,
            preferences,
        })
    }
}
impl Selection {
    pub fn preferences(&self) -> &Config {
        &self.preferences
    }
    pub fn receiver_to_verify(
        &self,
        active: &Config,
    ) -> Option<&prysm_webos_protocol::InstanceUrl> {
        match &self.target {
            CaptureTarget::Remote(remote) => (active.location != self.preferences.location
                || active.instance_url.as_ref() != Some(&remote.instance_url))
            .then_some(&remote.instance_url),
            #[cfg(feature = "embedded-prysm")]
            CaptureTarget::Embedded(_) => None,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Action {
    Spawn(CaptureTarget),
    Terminate,
    Kill,
}
// Status flags are independent fields in the existing Luna contract.
#[allow(clippy::struct_excessive_bools)]
pub struct Supervisor {
    pub config: Config,
    target: Option<CaptureTarget>,
    pub enabled: bool,
    // Remote enablement survives a round trip through the portal-owned mode.
    remote_enabled_before_embedded: bool,
    portal_url: Option<String>,
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
        let target = config.resolve().ok();
        Self {
            enabled: config.autostart && target.is_some(),
            remote_enabled_before_embedded: config.autostart && config.instance_url.is_some(),
            portal_url: None,
            target,
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
        let awake = self.power.as_deref() == Some("Active")
            || (self.hosts_portal() && self.power.as_deref() == Some("Screen Saver"));
        awake && self.target.is_some() && (self.enabled || self.hosts_portal())
    }

    pub fn hosts_portal(&self) -> bool {
        matches!(
            self.config.location,
            crate::config::ProcessingLocation::Embedded
        ) && self.target.is_some()
    }

    pub fn capture_allowed(&self) -> bool {
        self.enabled && self.power.as_deref() == Some("Active")
    }

    pub fn pause_capture(&mut self, now: Instant) {
        self.enabled = false;
        if !self.hosts_portal() {
            self.stop(now, true);
        }
    }

    /// Release the selected host as well as capture on service shutdown or cleanup.
    pub fn deactivate(&mut self, now: Instant) {
        self.enabled = false;
        self.target = None;
        self.stop(now, true);
    }

    pub fn contract(&self) -> SupervisorStatus {
        SupervisorStatus {
            enabled: self.enabled,
            power: self.power.clone(),
            sender_running: self.running,
            receiver_connected: self.connected,
            receiver_busy: self.busy,
            capturing: self.capturing,
            portal_url: self.portal_url.clone(),
            stopping: self.stopping,
            error: self.error.clone(),
            last_log: self.log.clone(),
            config: self.config.clone(),
            capabilities: Capabilities {
                hardware_h264: cfg!(feature = "hardware-h264"),
                embedded_portal: cfg!(feature = "embedded-prysm"),
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
        self.actions.push(Action::Spawn(
            self.target
                .clone()
                .expect("wanted requires a selected target"),
        ));
    }
    pub fn set_power(&mut self, power: Option<String>, now: Instant) {
        self.power = power;
        if !self.wanted() {
            self.stop(now, true);
        }
        self.reconcile(now);
    }
    pub fn start(&mut self, now: Instant) -> anyhow::Result<()> {
        self.target = Some(self.config.resolve()?);
        self.enabled = true;
        self.reconcile(now);
        Ok(())
    }
    pub fn configure(&mut self, selected: Selection, now: Instant) {
        if self.config.location != selected.preferences.location {
            if selected.preferences.location == crate::config::ProcessingLocation::Embedded {
                self.remote_enabled_before_embedded = self.enabled;
                self.enabled = true;
            } else {
                self.enabled = self.remote_enabled_before_embedded;
            }
        }
        self.config = selected.preferences;
        if self.target.as_ref() == Some(&selected.target) {
            return;
        }
        self.target = Some(selected.target);
        self.connected = false;
        self.capturing = false;
        self.log = None;
        self.error = None;
        self.busy = false;
        self.retry_delay = 1;
        self.stop(now, true);
        self.reconcile(now);
    }
    pub fn stop(&mut self, now: Instant, immediate: bool) {
        self.retry = None;
        self.portal_url = None;
        self.immediate |= immediate;
        if self.running && !self.stopping {
            self.stopping = true;
            self.encoder_at = None;
            self.video_reset_at = None;
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
        self.portal_url = None;
        self.stopping = false;
        self.connected = false;
        self.capturing = false;
        self.kill_at = None;
        self.video_reset_at = None;
        self.encoder_at = None;
        if self.wanted() && !self.immediate {
            if !success && !self.expected_exit {
                self.error
                    .get_or_insert_with(|| format!("Capturer exited: {description}"));
                self.retry_delay = MAX_RETRY_SECONDS;
            }
            self.retry = Some(now + Duration::from_secs(self.retry_delay));
            self.retry_delay = (self.retry_delay * 2).min(MAX_RETRY_SECONDS);
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
            Event::PortalReady { address } => {
                if self.hosts_portal()
                    && !address.ip().is_unspecified()
                    && !address.ip().is_loopback()
                    && !address.ip().is_multicast()
                    && address.port() != 0
                {
                    self.portal_url = Some(format!("http://{address}/"));
                    self.retry_delay = 1;
                }
            }
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
            Event::Capturing => {
                self.retry_delay = 1;
                self.capturing = true;
            }
            Event::CaptureIdle | Event::CapturePaused => self.capturing = false,
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
        if !self.stopping {
            self.log = Some(value);
        }
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
    #[cfg(feature = "embedded-prysm")]
    fn portal_is_reachable_with_capture_idle_paused_or_screen_saving_but_reaped_on_suspend() {
        let now = Instant::now();
        let mut state = Supervisor::new(Config {
            location: crate::config::ProcessingLocation::Embedded,
            ..Config::default()
        });
        assert!(!state.enabled, "cold start honors capture autostart");
        state.set_power(Some("Screen Saver".into()), now);
        assert!(matches!(
            state.actions().as_slice(),
            [Action::Spawn(CaptureTarget::Embedded(_))]
        ));
        assert!(!state.capture_allowed());
        for invalid in ["127.0.0.1:8787", "0.0.0.0:8787", "192.0.2.12:0"] {
            state.event(
                Event::PortalReady {
                    address: invalid.parse().unwrap(),
                },
                now,
            );
            assert_eq!(state.contract().portal_url, None);
        }
        state.event(
            Event::PortalReady {
                address: "192.0.2.12:8787".parse().unwrap(),
            },
            now,
        );
        assert_eq!(
            state.contract().portal_url.as_deref(),
            Some("http://192.0.2.12:8787/")
        );
        state.set_power(Some("Active".into()), now);
        assert!(!state.capture_allowed());
        state.start(now).unwrap();
        assert!(state.capture_allowed());
        state.pause_capture(now);
        state.tick(now);
        assert_eq!(state.actions(), []);
        assert!(state.contract().portal_url.is_some());
        state.set_power(Some("Suspend".into()), now);
        assert_eq!(state.actions(), [Action::Terminate]);
        assert_eq!(state.contract().portal_url, None);
        state.set_power(Some("Active".into()), now);
        assert_eq!(state.actions(), []);
        state.closed(true, "normal", now);
        assert!(matches!(
            state.actions().as_slice(),
            [Action::Spawn(CaptureTarget::Embedded(_))]
        ));
        assert!(!state.capture_allowed());
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
        assert_eq!(
            state.actions(),
            [Action::Spawn(state.config.resolve().unwrap())]
        );
        state.set_power(Some("Suspend".into()), now);
        assert_eq!(state.actions(), [Action::Terminate]);
        state.set_power(Some("Active".into()), now);
        assert_eq!(state.actions(), []);
        state.closed(true, "0", now);
        assert_eq!(
            state.actions(),
            [Action::Spawn(state.config.resolve().unwrap())]
        );
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
        state.configure(config.try_into().unwrap(), now);
        assert_eq!(state.actions(), [Action::Terminate]);
        state.closed(true, "0", now);
        assert_eq!(
            state.actions(),
            [Action::Spawn(state.config.resolve().unwrap())]
        );
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
        assert_eq!(
            state.actions(),
            [Action::Spawn(state.config.resolve().unwrap())]
        );
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
            assert_eq!(
                state.actions(),
                [Action::Spawn(state.config.resolve().unwrap())]
            );
            state.closed(false, "failed", now);
            state.enabled = false;
            state.stop(now, false);
            state.tick(now + Duration::from_secs(100));
            assert_eq!(state.actions(), []);
        }
    }
    #[test]
    fn busy_tv_retries_at_maximum_backoff_in_time_to_start_capture() {
        let mut now = Instant::now();
        let mut state = configured();
        state.set_power(Some("Active".into()), now);
        for seconds in [1, 2, 4, 5, 5] {
            assert_eq!(
                state.actions(),
                [Action::Spawn(state.config.resolve().unwrap())]
            );
            state.event(Event::ReceiverBusy, now);
            state.closed(true, "busy", now);
            state.tick(now + Duration::from_millis(seconds * 1000 - 1));
            assert_eq!(state.actions(), []);
            now += Duration::from_secs(seconds);
            state.tick(now);
        }
        assert_eq!(
            state.actions(),
            [Action::Spawn(state.config.resolve().unwrap())]
        );
        state.event(Event::Connected, now);
        state.event(Event::Capturing, now);
        assert!(state.contract().capturing);
        assert!(!state.contract().receiver_busy);
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
        assert_eq!(
            state.actions(),
            [Action::Spawn(state.config.resolve().unwrap())]
        );
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
        assert_eq!(
            state.actions(),
            [Action::Spawn(state.config.resolve().unwrap())]
        );
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
        assert_eq!(
            state.actions(),
            [Action::Spawn(state.config.resolve().unwrap())]
        );
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
        assert_eq!(
            state.actions(),
            [Action::Spawn(state.config.resolve().unwrap())]
        );
        state.tick(now + Duration::from_secs(27));
        assert_eq!(state.actions(), []);
    }
}
