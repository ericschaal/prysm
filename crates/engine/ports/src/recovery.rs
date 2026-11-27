//! What crosses a layer when a failure leaves it ([`Escalation`], carried by
//! [`crate::Error::escalation`]).
//!
//! Each owner of an operation defines, next to it, its own closed set of
//! reasons to retry (each with why it converges and when it gives up) and of
//! signals to wait for; neither ever leaves that owner.
use serde::{Deserialize, Serialize};

/// What is left once a failure leaves the layer that met it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "step", content = "fix", rename_all = "snake_case")]
pub enum Escalation {
    /// Only a new process recovers (a session that did not stop or did not
    /// confirm its release): whatever starts the process starts one, once.
    /// If that one faults again before it has worked, it is
    /// `Ask(Fix::TryAgain)`. A process that nothing restarts fails fast.
    Restart,
    /// The device did not answer: it may be off or away from the network.
    /// Nothing inside the process fixes it and nothing announces its
    /// return, so the person is told and a reader may also try the device
    /// again at an interval until it answers or the person acts. A reader
    /// that does not try again fails fast.
    Unanswered,
    /// The person must act: shown with this step, and resumed only by it.
    Ask(Fix),
}

impl Escalation {
    /// Fail fast: nothing recovers this automatically; the person is told
    /// and decides whether to try again.
    pub const FAIL_FAST: Self = Self::Ask(Fix::TryAgain);

    /// The person's next step on a desktop host, which restarts and retries
    /// nothing, so `Restart` is the person's too. `Unanswered` reaches a
    /// desktop host only where nothing waits for the lights: the idle of a
    /// session the target moved away from.
    #[must_use]
    pub const fn next_step(self) -> &'static str {
        match self {
            Self::Restart => "restart Prysm",
            Self::Unanswered => "check that the lights are on and on the network",
            Self::Ask(Fix::TryAgain) => "try again",
            Self::Ask(Fix::ChangeSettings) => "change the settings",
            Self::Ask(Fix::SetUp) => "run the setup again",
            Self::Ask(Fix::Unsupported) => "not supported on this device",
            Self::Ask(Fix::RestartTv) => "restart the TV",
        }
    }
}

/// The person's next step. `try_again`: start again. `change_settings`:
/// change the settings (in the portal); a save that validates replaces them.
/// `set_up`: set up the service again: it lacks a permission it needs.
/// `unsupported`: nothing makes this work on this TV. `restart_tv`: restart
/// the TV: a worker that did not end may still hold the capture. Open: an
/// app reads a fix it does not know as `try_again`.
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fix {
    /// Start again.
    TryAgain,
    /// Change the settings; a save that validates replaces them.
    ChangeSettings,
    /// Run the installation's setup again: it lacks a permission it needs.
    SetUp,
    /// Nothing makes this work on this device.
    Unsupported,
    /// Restart the device: a process that did not end may still hold what
    /// a new one needs, and only the device's restart ends it.
    RestartTv,
}

impl Fix {
    pub const ALL: [Self; 5] = [
        Self::TryAgain,
        Self::ChangeSettings,
        Self::SetUp,
        Self::Unsupported,
        Self::RestartTv,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every value's JSON, and that it reads back as that value.
    #[test]
    fn escalations_and_fixes_have_these_json_shapes() {
        let escalations = [
            (Escalation::Restart, r#"{"step":"restart"}"#),
            (Escalation::Unanswered, r#"{"step":"unanswered"}"#),
            (
                Escalation::Ask(Fix::TryAgain),
                r#"{"step":"ask","fix":"try_again"}"#,
            ),
            (
                Escalation::Ask(Fix::ChangeSettings),
                r#"{"step":"ask","fix":"change_settings"}"#,
            ),
            (
                Escalation::Ask(Fix::SetUp),
                r#"{"step":"ask","fix":"set_up"}"#,
            ),
            (
                Escalation::Ask(Fix::Unsupported),
                r#"{"step":"ask","fix":"unsupported"}"#,
            ),
            (
                Escalation::Ask(Fix::RestartTv),
                r#"{"step":"ask","fix":"restart_tv"}"#,
            ),
        ];
        for (escalation, json) in escalations {
            assert_eq!(serde_json::to_string(&escalation).unwrap(), json);
            assert_eq!(
                serde_json::from_str::<Escalation>(json).unwrap(),
                escalation
            );
        }
        let fixes = [
            (Fix::TryAgain, r#""try_again""#),
            (Fix::ChangeSettings, r#""change_settings""#),
            (Fix::SetUp, r#""set_up""#),
            (Fix::Unsupported, r#""unsupported""#),
            (Fix::RestartTv, r#""restart_tv""#),
        ];
        assert_eq!(fixes.map(|(fix, _)| fix), Fix::ALL);
        for (fix, json) in fixes {
            assert_eq!(serde_json::to_string(&fix).unwrap(), json);
            assert_eq!(serde_json::from_str::<Fix>(json).unwrap(), fix);
        }
    }

    #[test]
    fn each_escalation_names_one_next_step() {
        let steps = [
            (Escalation::Restart, "restart Prysm"),
            (
                Escalation::Unanswered,
                "check that the lights are on and on the network",
            ),
            (Escalation::Ask(Fix::TryAgain), "try again"),
            (Escalation::Ask(Fix::ChangeSettings), "change the settings"),
            (Escalation::Ask(Fix::SetUp), "run the setup again"),
            (
                Escalation::Ask(Fix::Unsupported),
                "not supported on this device",
            ),
            (Escalation::Ask(Fix::RestartTv), "restart the TV"),
        ];
        for (escalation, step) in steps {
            assert_eq!(escalation.next_step(), step, "{escalation:?}");
        }
    }
}
