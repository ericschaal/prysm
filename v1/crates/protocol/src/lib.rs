//! JSON values Prysm surfaces exchange with their clients.
//!
//! Settings, targets and revisions are `prysm_ports` types and serialize as
//! they do in the settings file. This crate adds only what ports cannot carry:
//! a serializable session state and failure, and the request/response bodies.
//! It has no HTTP or runtime dependencies.
use prysm_ports::Input as InputRate;
use prysm_ports::OutputIssue as PortsOutputIssue;
use prysm_ports::{
    Calibration, Error, LightingTarget, Revision, SessionPhase, SessionState, Settings, Snapshot,
};
use serde::{Deserialize, Serialize};
use std::error::Error as _;

#[cfg(feature = "schema")]
pub mod schema;

mod preview;
pub use preview::{PreviewColor, PreviewColors, PreviewEvent, PreviewFrame, PreviewViewport};

/// The observable runtime state, sent whole on every change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct State {
    #[cfg_attr(feature = "schema", schema(value_type = schema::LightingTargetSchema))]
    pub target: LightingTarget,
    pub session: Session,
    /// Frames arriving from the source, measured about once a second; absent
    /// while no session runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schema(nullable = false))]
    pub input: Option<Input>,
    /// Why the lights are not showing Prysm's colours while lighting runs,
    /// from the output's own checks (about once a second); absent while they
    /// show them, while the output cannot tell, and while no session runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schema(nullable = false))]
    pub output_issue: Option<OutputIssue>,
    /// The calibration pattern the TV shows, set by `PUT /api/v1/calibration`;
    /// absent while not calibrating. Never saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "schema",
        schema(value_type = Option<schema::CalibrationSchema>, nullable = false)
    )]
    pub calibration: Option<Calibration>,
    #[cfg_attr(feature = "schema", schema(value_type = schema::SettingsSchema))]
    pub settings: Settings,
    /// Pass back as `expected` when replacing settings.
    #[cfg_attr(feature = "schema", schema(value_type = u64))]
    pub revision: Revision,
    /// Last lighting failure; cleared when lighting next starts.
    #[cfg_attr(feature = "schema", schema(required = true))]
    pub failure: Option<Failure>,
    /// Surfaces that stopped. Lighting continues without them.
    pub surface_failures: Vec<Failure>,
}

impl From<&Snapshot> for State {
    fn from(snapshot: &Snapshot) -> Self {
        Self {
            target: snapshot.target,
            session: snapshot.session.into(),
            input: snapshot.input.map(Input::from),
            output_issue: snapshot.output_issue.map(OutputIssue::from),
            calibration: snapshot.calibration,
            settings: snapshot.settings.clone(),
            revision: snapshot.revision,
            failure: snapshot.failure.as_ref().map(Failure::from),
            surface_failures: snapshot
                .surface_failures
                .iter()
                .map(Failure::from)
                .collect(),
        }
    }
}

/// Whether frames are arriving, and how fast.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Input {
    /// Frames per second over the last second, to one decimal.
    #[cfg_attr(feature = "schema", schema(minimum = 0, example = 29.9))]
    pub fps: f32,
    /// At least one frame arrived in the last second. `false` right after
    /// lighting starts and while the source is quiet (no signal, no sender).
    pub receiving: bool,
}

impl From<InputRate> for Input {
    fn from(input: InputRate) -> Self {
        Self {
            fps: input.fps,
            receiving: input.receiving,
        }
    }
}

/// Why the lights are not showing Prysm's colours. Prysm keeps sending; they
/// follow again once the device takes the colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum OutputIssue {
    /// The device was switched off (WLED: power off).
    Off,
    /// The device's own override is on (WLED: live override).
    Overridden,
    /// The device is not taking the colours (WLED: not in live mode, or no
    /// DDP arriving).
    NotLive,
    /// Another sender's colours reach the device.
    OtherSender,
    /// The device does not answer state requests.
    Unreachable,
}

impl From<PortsOutputIssue> for OutputIssue {
    fn from(issue: PortsOutputIssue) -> Self {
        match issue {
            PortsOutputIssue::Off => Self::Off,
            PortsOutputIssue::Overridden => Self::Overridden,
            PortsOutputIssue::NotLive => Self::NotLive,
            PortsOutputIssue::OtherSender => Self::OtherSender,
            PortsOutputIssue::Unreachable => Self::Unreachable,
        }
    }
}

/// What the runtime is actually doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum Session {
    Stopped,
    Running,
    /// The output shows its own effect.
    Native,
    /// A session did not stop in time; only a restart recovers.
    Faulted {
        phase: Phase,
    },
}

impl From<SessionState> for Session {
    fn from(state: SessionState) -> Self {
        match state {
            SessionState::Stopped => Self::Stopped,
            SessionState::Running => Self::Running,
            SessionState::Native => Self::Native,
            SessionState::Faulted(phase) => Self::Faulted {
                phase: phase.into(),
            },
        }
    }
}

/// Where a faulted session stopped making progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum Phase {
    Running,
    ClosingSource,
    IdlingOutput,
    Finishing,
}

impl From<SessionPhase> for Phase {
    fn from(phase: SessionPhase) -> Self {
        match phase {
            SessionPhase::Running => Self::Running,
            SessionPhase::ClosingSource => Self::ClosingSource,
            SessionPhase::IdlingOutput => Self::IdlingOutput,
            SessionPhase::Finished => Self::Finishing,
        }
    }
}

/// A stable failure body: the message a user can act on. An adapter failure
/// carries its cause chain (`context: cause: cause`), since the cause is what
/// names the problem (a device's error, a remote sender's reason).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Failure {
    pub kind: FailureKind,
    pub message: String,
    /// The rejected setting as a dotted path, such as `output.address`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schema(nullable = false))]
    pub field: Option<String>,
    /// The revision to retry against after `stale_revision`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schema(value_type = Option<u64>, nullable = false))]
    pub current: Option<Revision>,
}

impl Failure {
    #[must_use]
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            field: None,
            current: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum FailureKind {
    /// Settings were rejected; see `field`.
    Invalid,
    /// Settings changed since `expected`; see `current`.
    StaleRevision,
    /// The runtime is faulted and accepts nothing until restarted.
    Faulted,
    /// A device or adapter failed.
    Adapter,
    /// The runtime or surface is stopping or stopped.
    Unavailable,
    /// The command did not answer in time; its outcome shows in the state.
    Timeout,
    /// An unexpected internal failure.
    Internal,
    Forbidden,
    NotFound,
    MethodNotAllowed,
    /// The request was malformed.
    BadRequest,
    PayloadTooLarge,
    UnsupportedMediaType,
    /// Too many concurrent requests or streams.
    Busy,
}

impl From<&Error> for Failure {
    fn from(error: &Error) -> Self {
        let kind = match error {
            Error::Invalid(_) | Error::Decode { .. } => FailureKind::Invalid,
            Error::StaleRevision { .. } => FailureKind::StaleRevision,
            Error::Faulted(_) => FailureKind::Faulted,
            Error::Adapter { .. } => FailureKind::Adapter,
            Error::Closed => FailureKind::Unavailable,
            _ => FailureKind::Internal,
        };
        let mut failure = Self::new(kind, error.to_string());
        match error {
            Error::Adapter { .. } => failure.message = prysm_ports::chain(error),
            Error::Invalid(invalid) => {
                failure.field = Some(invalid.field.clone());
                failure.message.clone_from(&invalid.reason);
            }
            // The decoder's message names the field the user got wrong.
            Error::Decode { .. } => {
                if let Some(source) = error.source() {
                    failure.message = format!("{error}: {source}");
                }
            }
            Error::StaleRevision { current, .. } => failure.current = Some(*current),
            _ => {}
        }
        failure
    }
}

/// `PUT /api/v1/target`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct TargetRequest {
    #[cfg_attr(feature = "schema", schema(value_type = schema::LightingTargetSchema))]
    pub target: LightingTarget,
}

/// `PUT /api/v1/settings`: replace all settings if `expected` is current.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct SettingsRequest {
    #[cfg_attr(feature = "schema", schema(value_type = u64))]
    pub expected: Revision,
    #[cfg_attr(feature = "schema", schema(value_type = schema::SettingsSchema))]
    pub settings: Settings,
}

/// Reply to a successful settings replacement. Applying them may still fail;
/// that failure appears in the state's `failure`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct SettingsReplaced {
    #[cfg_attr(feature = "schema", schema(value_type = u64))]
    pub revision: Revision,
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_ports::ValidationError;
    use serde_json::json;

    #[test]
    fn state_serializes_ports_values_unchanged() {
        let snapshot = Snapshot {
            session: SessionState::Faulted(SessionPhase::ClosingSource),
            revision: Revision(3),
            failure: Some(Error::other("device unplugged")),
            ..Snapshot::default()
        };
        let value = serde_json::to_value(State::from(&snapshot)).unwrap();
        assert_eq!(value["target"], "off");
        assert_eq!(value["revision"], 3);
        assert_eq!(
            value["session"],
            json!({ "state": "faulted", "phase": "closing_source" })
        );
        assert_eq!(
            value["failure"],
            json!({ "kind": "adapter", "message": "device unplugged" })
        );
        assert_eq!(
            value["settings"],
            serde_json::to_value(&snapshot.settings).unwrap()
        );
        assert!(value.get("input").is_none(), "absent while stopped");
        assert!(value.get("output_issue").is_none(), "absent while stopped");
        assert!(
            value.get("calibration").is_none(),
            "absent while not calibrating"
        );
        let back: State = serde_json::from_value(value).unwrap();
        assert_eq!(back.session, snapshot.session.into());

        let running = Snapshot {
            session: SessionState::Running,
            input: Some(InputRate {
                fps: 29.9,
                receiving: true,
            }),
            output_issue: Some(PortsOutputIssue::OtherSender),
            ..Snapshot::default()
        };
        let running = Snapshot {
            calibration: Some(Calibration {
                pattern: prysm_ports::Pattern::BluePair,
                uncorrected: true,
            }),
            ..running
        };
        let value = serde_json::to_value(State::from(&running)).unwrap();
        assert_eq!(
            value["calibration"],
            json!({ "pattern": "blue-pair", "uncorrected": true })
        );
        assert_eq!(
            serde_json::from_value::<State>(value.clone())
                .unwrap()
                .calibration,
            running.calibration
        );
        assert_eq!(value["input"]["receiving"], true);
        assert_eq!(value["output_issue"], "other_sender");
        assert!((value["input"]["fps"].as_f64().unwrap() - 29.9).abs() < 1e-4);
    }

    #[test]
    fn failures_keep_field_current_revision_and_decode_reason() {
        let invalid = Failure::from(&Error::from(ValidationError::new(
            "output.leds",
            "too many",
        )));
        assert_eq!(invalid.kind, FailureKind::Invalid);
        assert_eq!(invalid.field.as_deref(), Some("output.leds"));
        assert_eq!(invalid.message, "too many");

        let stale = Failure::from(&Error::StaleRevision {
            expected: Revision(1),
            current: Revision(2),
        });
        assert_eq!(stale.current, Some(Revision(2)));

        let source = serde_json::from_str::<u8>("\"x\"").unwrap_err();
        let decode = Failure::from(&Error::Decode {
            kind: "wled".into(),
            source: std::sync::Arc::new(source),
        });
        assert_eq!(decode.kind, FailureKind::Invalid);
        assert!(
            decode
                .message
                .starts_with("cannot decode `wled` settings: invalid type")
        );
    }

    /// The portal shows what went wrong, not only where: an adapter failure
    /// keeps its causes (here a remote sender's reason).
    #[test]
    fn adapter_failures_carry_their_cause_chain() {
        #[derive(Debug)]
        struct Cause;
        impl std::fmt::Display for Cause {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("hardware H.264 encoder cannot start")
            }
        }
        impl std::error::Error for Cause {}
        #[derive(Debug)]
        struct Outer(Cause);
        impl std::fmt::Display for Outer {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("cannot encode frame")
            }
        }
        impl std::error::Error for Outer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }
        let failure = Failure::from(&Error::adapter("the sender cannot stream", Outer(Cause)));
        assert_eq!(failure.kind, FailureKind::Adapter);
        assert_eq!(
            failure.message,
            "the sender cannot stream: cannot encode frame: hardware H.264 encoder cannot start"
        );
        assert_eq!(failure.field, None);
        let bare = Failure::from(&Error::other("no frames"));
        assert_eq!(bare.message, "no frames");
    }

    #[test]
    fn requests_reject_unknown_fields() {
        let request: SettingsRequest =
            serde_json::from_value(json!({ "expected": 0, "settings": {} })).unwrap();
        assert_eq!(request.settings, Settings::default());
        assert!(serde_json::from_value::<TargetRequest>(json!({ "target": "on" })).is_err());
        assert!(
            serde_json::from_value::<TargetRequest>(json!({ "target": "off", "id": 1 })).is_err()
        );
    }
}
