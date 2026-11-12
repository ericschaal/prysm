//! JSON values Prysm surfaces exchange with their clients.
//!
//! Settings, targets and revisions are `prysm_ports` types and serialize as
//! they do in the settings file. This crate adds only what ports cannot carry:
//! a serializable session state and failure, and the request/response bodies.
//! It has no HTTP or runtime dependencies.
use prysm_ports::{
    Error, LightingTarget, Revision, SessionPhase, SessionState, Settings, Snapshot,
};
use serde::{Deserialize, Serialize};
use std::error::Error as _;

#[cfg(feature = "schema")]
pub mod schema;

/// The observable runtime state, sent whole on every change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct State {
    #[cfg_attr(feature = "schema", schema(value_type = schema::LightingTargetSchema))]
    pub target: LightingTarget,
    pub session: Session,
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

/// A stable failure body. Local source chains stay in the process; only the
/// message a user can act on is sent.
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
    Unauthorized,
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
        let back: State = serde_json::from_value(value).unwrap();
        assert_eq!(back.session, snapshot.session.into());
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
