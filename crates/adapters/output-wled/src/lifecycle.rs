//! WLED readiness, display ownership, and handoff decisions.

use crate::observe::Status;
use prysm_ports::{
    Error, Escalation, OutputDisplay, OutputIssue, OutputObservation, Power, Readiness,
};
use serde_json::{Value, json};
use std::net::IpAddr;

/// A successfully decoded WLED state reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WledObservation {
    pub(crate) power: Power,
    pub(crate) overridden: bool,
    pub(crate) ddp: bool,
    pub(crate) realtime: bool,
    pub(crate) sender: Option<IpAddr>,
}

impl WledObservation {
    /// Decode the same `/json/si` reply used by the streaming observer.
    ///
    /// # Errors
    /// The payload is malformed or lacks the required WLED state fields.
    pub fn parse(body: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice::<Status>(body).map(|status| status.observation())
    }

    /// Report physical power and display facts without exposing WLED details.
    #[must_use]
    pub const fn output(self) -> OutputObservation {
        let display = if self.overridden {
            OutputDisplay::Override
        } else if self.ddp {
            OutputDisplay::Live
        } else {
            OutputDisplay::Idle
        };
        OutputObservation {
            power: self.power,
            display,
        }
    }

    /// Interpret a status while streaming, after the live command succeeded.
    /// Supply the address this session knows WLED sees for its own packets.
    #[must_use]
    pub fn during_stream(self, own_sender: Option<IpAddr>) -> StreamDisplay {
        match (
            self.power,
            self.overridden,
            self.realtime,
            self.sender,
            own_sender,
        ) {
            (_, true, _, _, _) => StreamDisplay::HandOff,
            (Power::On, false, true, Some(sender), Some(ours)) if sender != ours => {
                StreamDisplay::Contended
            }
            (Power::On, false, true, Some(_), Some(_)) if self.ddp => StreamDisplay::Ours,
            _ => StreamDisplay::Unknown,
        }
    }

    pub(crate) fn issue(self, own_sender: Option<IpAddr>) -> Option<OutputIssue> {
        if self.power == Power::Off {
            return Some(OutputIssue::Off);
        }
        if self.overridden {
            return Some(OutputIssue::Overridden);
        }
        if !self.realtime {
            return Some(OutputIssue::NotLive);
        }
        (self.during_stream(own_sender) == StreamDisplay::Contended)
            .then_some(OutputIssue::OtherSender)
    }
}

/// The action or diagnostic from a streaming observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamDisplay {
    /// WLED explicitly overrides the stream. Stop DDP and hand off to Effects.
    HandOff,
    /// The observed sender matches this session.
    Ours,
    /// Another DDP sender controls the display.
    Contended,
    /// The observation cannot establish the current controller.
    Unknown,
}

/// Classify an adapter failure from its existing typed recovery decision.
/// Effects retired by their owner never deliver a failure here.
#[must_use]
pub fn readiness_failure(error: Error) -> Readiness<Error> {
    match error.escalation() {
        Escalation::Unanswered => Readiness::Waiting(error),
        Escalation::Ask(_) | Escalation::Restart => Readiness::Blocked(error),
    }
}

/// Permission carried by the request that starts Screen sync.
pub(crate) enum ScreenSyncRequest {
    Explicit,
    Resume(WledObservation),
}

/// Prepare WLED for DDP once capture is open. DDP packets start a finite
/// realtime lock, so preparing a session with no frames cannot freeze WLED
/// in JSON live mode.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ScreenSyncStart {
    HandOff(OutputObservation),
    Prepare(Value),
}

impl ScreenSyncRequest {
    pub(crate) fn start(self) -> ScreenSyncStart {
        match self {
            Self::Explicit => ScreenSyncStart::Prepare(json!({"on": true, "lor": 0, "tt": 0})),
            Self::Resume(state) if state.overridden => ScreenSyncStart::HandOff(state.output()),
            Self::Resume(_) => ScreenSyncStart::Prepare(json!({"on": true, "tt": 0})),
        }
    }
}

/// Power WLED for Effects when needed, preserving its display settings.
pub(crate) fn effects_power_command(status: WledObservation) -> Option<Value> {
    (status.power == Power::Off).then(|| json!({"on": true, "tt": 0}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_ports::{AdapterLifecycle, Availability, Fix};

    const OURS: IpAddr = IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

    fn state(on: bool, lor: u8, lm: &str, lip: &str) -> WledObservation {
        let body = json!({
            "state": {"on": on, "lor": lor},
            "info": {"live": true, "lm": lm, "lip": lip},
        });
        WledObservation::parse(&serde_json::to_vec(&body).unwrap()).unwrap()
    }

    #[test]
    fn startup_reports_override_even_while_off_and_recognizes_only_ddp_as_live() {
        for lor in [1, 2] {
            assert_eq!(
                state(false, lor, "DDP", "").output(),
                OutputObservation {
                    power: Power::Off,
                    display: OutputDisplay::Override,
                },
            );
        }
        assert_eq!(
            state(true, 0, "DDP", "").output().display,
            OutputDisplay::Live
        );
        for mode in ["", "UDP"] {
            assert_eq!(
                state(true, 0, mode, "").output().display,
                OutputDisplay::Idle
            );
        }
        assert!(WledObservation::parse(br#"{"state":{"on":true}}"#).is_err());
    }

    #[test]
    fn explicit_open_clears_either_override_and_resume_preserves_it() {
        let explicit = ScreenSyncRequest::Explicit.start();
        assert_eq!(
            explicit,
            ScreenSyncStart::Prepare(json!({"on": true, "lor": 0, "tt": 0}))
        );
        for lor in [1, 2] {
            let seen = state(true, lor, "DDP", "");
            assert_eq!(
                ScreenSyncRequest::Resume(seen).start(),
                ScreenSyncStart::HandOff(seen.output())
            );
            assert_eq!(seen.during_stream(Some(OURS)), StreamDisplay::HandOff);
        }

        let idle = state(true, 0, "", "");
        assert_eq!(
            ScreenSyncRequest::Resume(idle).start(),
            ScreenSyncStart::Prepare(json!({"on": true, "tt": 0}))
        );
        let external = state(true, 0, "DDP", "127.0.0.2");
        assert_eq!(external.during_stream(Some(OURS)), StreamDisplay::Contended);
        assert_eq!(idle.during_stream(Some(OURS)), StreamDisplay::Unknown);
    }

    #[test]
    fn release_requires_control_and_effects_power_leaves_settings_untouched() {
        let ours = state(true, 0, "DDP", "127.0.0.1");
        let external = state(true, 0, "DDP", "127.0.0.2");
        let override_state = state(true, 2, "DDP", "127.0.0.1");
        let off = state(false, 0, "", "");
        assert_eq!(ours.during_stream(Some(OURS)), StreamDisplay::Ours);
        assert_eq!(ours.during_stream(None), StreamDisplay::Unknown);
        assert_eq!(external.during_stream(Some(OURS)), StreamDisplay::Contended);
        assert_eq!(
            override_state.during_stream(Some(OURS)),
            StreamDisplay::HandOff
        );
        assert_eq!(
            effects_power_command(off),
            Some(json!({"on": true, "tt": 0}))
        );
        assert!(effects_power_command(override_state).is_none());
    }

    #[test]
    fn unreachable_off_reports_waiting_after_local_release_and_monitor_can_recover() {
        let mut adapter = AdapterLifecycle::default();
        adapter.start(Readiness::Ready);
        let claim = adapter.claim().unwrap();
        let release = adapter.begin_release(&claim).unwrap();
        let unanswered = Error::other_escalating("WLED timed out", Escalation::Unanswered);
        adapter.observe(readiness_failure(unanswered));
        assert_eq!(adapter.released(&release), Some(Availability::Waiting));
        assert_eq!(adapter.availability(), Availability::Waiting);

        adapter.observe(Readiness::Ready);
        assert_eq!(adapter.availability(), Availability::Ready);

        let invalid =
            Error::other_escalating("unexpected reply", Escalation::Ask(Fix::ChangeSettings));
        adapter.observe(readiness_failure(invalid));
        assert_eq!(adapter.availability(), Availability::Blocked);
    }
}
