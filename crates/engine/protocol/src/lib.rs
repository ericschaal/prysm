//! JSON values Prysm surfaces exchange with their clients.
//!
//! Settings, targets, revisions, inputs and device reports are `prysm_ports`
//! types and serialize as ports does. This crate adds only what ports cannot
//! carry: the product version, a tagged session state with its failure, a
//! failure body, and the request/response bodies. It has no HTTP or runtime
//! dependencies.
use prysm_ports::{
    CalibrationPattern, DeviceReport, Error, Input, LightingTarget, Revision, Section,
    SessionPhase, SessionState, Settings, Snapshot,
};
use serde::{Deserialize, Serialize};

#[cfg(feature = "schema")]
pub mod schema;

mod colors;
mod preview;
pub use colors::{COLOR_SEGMENTS, ColorsEvent, LedColor, LedColors};
pub use preview::{PreviewEvent, PreviewFrame, PreviewViewport};

/// The observable runtime state, sent whole on every change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct State {
    /// The Prysm product version, such as `1.0.0` or `1.0.0-rc.1`.
    #[cfg_attr(feature = "schema", schema(example = "1.0.0"))]
    pub version: String,
    #[cfg_attr(feature = "schema", schema(value_type = schema::LightingTargetSchema))]
    pub target: LightingTarget,
    pub session: Session,
    /// Frames arriving from the source, measured about once a second; absent
    /// while no session runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "schema",
        schema(value_type = Option<schema::InputSchema>, nullable = false)
    )]
    pub input: Option<Input>,
    /// What the lights say about themselves, from the output's own checks
    /// (about once a second). Empty while no session holds the output: no
    /// session, or one that has no frame yet or released the output after 3 s
    /// without video; `session.state` and `input.receiving` say which.
    #[cfg_attr(feature = "schema", schema(value_type = schema::DeviceReportSchema))]
    pub device: DeviceReport,
    /// The calibration pattern the TV shows, set by `PUT /api/v1/calibration`;
    /// absent while not calibrating. Never saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "schema",
        schema(value_type = Option<schema::CalibrationPatternSchema>, nullable = false)
    )]
    pub calibration: Option<CalibrationPattern>,
    #[cfg_attr(feature = "schema", schema(value_type = schema::SettingsSchema))]
    pub settings: Settings,
    /// Pass back as `expected` when replacing settings.
    #[cfg_attr(feature = "schema", schema(value_type = u64))]
    pub revision: Revision,
    /// Surfaces that stopped. Lighting continues without them.
    pub surface_failures: Vec<Failure>,
}

impl From<&Snapshot> for State {
    fn from(snapshot: &Snapshot) -> Self {
        Self {
            version: prysm_version::VERSION.to_owned(),
            target: snapshot.target,
            session: (&snapshot.session).into(),
            input: snapshot.input,
            device: snapshot.device,
            calibration: snapshot.calibration,
            settings: snapshot.settings.clone(),
            revision: snapshot.revision,
            surface_failures: snapshot
                .surface_failures
                .iter()
                .map(Failure::from)
                .collect(),
        }
    }
}

/// What the runtime is actually doing. Screen sync carries no failure: it
/// runs on valid settings and has reached its target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum Session {
    /// No session; the lights are not driven.
    Stopped {
        /// Why the target is not reached, why the lights may not be off, or
        /// that the saved settings do not validate; absent while none does.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schema", schema(nullable = false))]
        failure: Option<LightingFailure>,
    },
    /// The lights follow the screen.
    ScreenSync,
    /// The lights show their own effects.
    Effects {
        /// Why screen sync, asked from the effects, did not start, or that
        /// the saved settings do not validate; absent while none does.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schema", schema(nullable = false))]
        failure: Option<LightingFailure>,
    },
    /// A session did not stop in time, or did not confirm its source's
    /// release; only a restart recovers.
    Faulted {
        /// What did not finish, for people.
        message: String,
    },
}

impl From<&SessionState> for Session {
    fn from(state: &SessionState) -> Self {
        let failure = || state.failure().map(LightingFailure::from);
        match state {
            SessionState::Stopped { .. } => Self::Stopped { failure: failure() },
            SessionState::ScreenSync => Self::ScreenSync,
            SessionState::Effects { .. } => Self::Effects { failure: failure() },
            SessionState::Faulted(phase) => Self::Faulted {
                message: faulted_message(*phase).to_owned(),
            },
        }
    }
}

/// What a faulted session did not finish, in words. The phase itself is
/// logged, not sent.
fn faulted_message(phase: SessionPhase) -> &'static str {
    match phase {
        SessionPhase::Running => "Lighting did not stop in time.",
        SessionPhase::ClosingSource => "The video source did not close in time.",
        SessionPhase::IdlingOutput => "The lights did not switch off in time.",
        SessionPhase::Finishing => "Lighting did not finish stopping in time.",
        SessionPhase::SourceUnreleased => "The video source did not confirm it was released.",
    }
}

/// Why the lighting is as it is: the failure a session state carries. An
/// adapter failure carries its cause chain (`context: cause: cause`), since
/// the cause is what names the problem (a device's error, a remote sender's
/// reason).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum LightingFailure {
    /// The saved settings do not validate, or cannot be read.
    Invalid {
        message: String,
        /// The rejected setting as a dotted path, such as `output.address`;
        /// absent when the settings as a whole are at fault.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schema", schema(nullable = false))]
        field: Option<String>,
    },
    /// A device or adapter failed.
    Adapter {
        message: String,
        /// Which side failed, when the engine knows.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schema", schema(nullable = false))]
        section: Option<FailureSection>,
    },
    /// An unexpected internal failure.
    Internal { message: String },
}

impl From<&Error> for LightingFailure {
    fn from(error: &Error) -> Self {
        match Failure::from(error) {
            Failure::Invalid { message, field } => Self::Invalid { message, field },
            Failure::Adapter { message, section } => Self::Adapter { message, section },
            // The engine keeps no other error in a session: a stale
            // revision and a pattern without screen sync are replies, and a
            // fault is its own state.
            other => Self::Internal {
                message: other.message().to_owned(),
            },
        }
    }
}

/// A failure reply (any non-2xx), tagged by `kind`: the message a user can
/// act on, with what the kind needs to act on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum Failure {
    /// Settings were rejected.
    Invalid {
        message: String,
        /// The rejected setting as a dotted path, such as `output.address`;
        /// absent when no one field is at fault.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schema", schema(nullable = false))]
        field: Option<String>,
    },
    /// Settings changed since `expected`.
    StaleRevision {
        message: String,
        /// The revision to retry against.
        #[cfg_attr(feature = "schema", schema(value_type = u64))]
        current: Revision,
    },
    /// A device or adapter failed. Its message carries the cause chain.
    Adapter {
        message: String,
        /// Which side failed, when the engine knows: the capture source or
        /// the output.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schema", schema(nullable = false))]
        section: Option<FailureSection>,
    },
    /// Calibration patterns need the `screen_sync` target.
    NeedsScreenSync {
        message: String,
    },
    /// The runtime is faulted and accepts nothing until restarted.
    Faulted {
        message: String,
    },
    /// The runtime or surface is stopping or stopped.
    Unavailable {
        message: String,
    },
    /// An unexpected internal failure.
    Internal {
        message: String,
    },
    Forbidden {
        message: String,
    },
    NotFound {
        message: String,
    },
    MethodNotAllowed {
        message: String,
    },
    /// The request was malformed.
    BadRequest {
        message: String,
    },
    PayloadTooLarge {
        message: String,
    },
    UnsupportedMediaType {
        message: String,
    },
    /// Too many concurrent requests or streams.
    Busy {
        message: String,
    },
}

impl Failure {
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::Invalid { message, .. }
            | Self::StaleRevision { message, .. }
            | Self::Adapter { message, .. }
            | Self::NeedsScreenSync { message }
            | Self::Faulted { message }
            | Self::Unavailable { message }
            | Self::Internal { message }
            | Self::Forbidden { message }
            | Self::NotFound { message }
            | Self::MethodNotAllowed { message }
            | Self::BadRequest { message }
            | Self::PayloadTooLarge { message }
            | Self::UnsupportedMediaType { message }
            | Self::Busy { message } => message,
        }
    }
}

/// The side of the signal path a failure came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum FailureSection {
    /// The video source: opening it or reading frames.
    Capture,
    /// The lights: opening, writing or idling the output.
    Output,
}

impl From<&Error> for Failure {
    fn from(error: &Error) -> Self {
        let message = error.to_string();
        match error {
            Error::Invalid(invalid) => Self::Invalid {
                message: invalid.reason.clone(),
                field: Some(invalid.field.clone()),
            },
            // No one field is at fault; the store's cause chain says why.
            Error::UnusableSettings(_) => Self::Invalid {
                message: prysm_ports::chain(error),
                field: None,
            },
            // The decoder's message names the field the user got wrong.
            Error::Decode { source, .. } => Self::Invalid {
                message: format!("{message}: {source}"),
                field: None,
            },
            Error::Adapter { .. } => Self::Adapter {
                message: prysm_ports::chain(error),
                section: match error.section() {
                    Some(Section::Capture) => Some(FailureSection::Capture),
                    Some(Section::Output) => Some(FailureSection::Output),
                    Some(Section::Processing) | None => None,
                },
            },
            Error::StaleRevision { current, .. } => Self::StaleRevision {
                message,
                current: *current,
            },
            Error::NeedsScreenSync => Self::NeedsScreenSync { message },
            Error::Faulted(_) => Self::Faulted { message },
            Error::Panicked(_) => Self::Internal { message },
            Error::Closed => Self::Unavailable { message },
        }
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
/// that failure appears in the state's `session`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct SettingsReplaced {
    #[cfg_attr(feature = "schema", schema(value_type = u64))]
    pub revision: Revision,
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_ports::{DeviceBrightness, OutputIssue, ValidationError};
    use serde_json::json;

    #[test]
    fn state_serializes_ports_values_unchanged() {
        let snapshot = Snapshot {
            session: SessionState::Stopped {
                failure: Some(Error::other("device unplugged")),
            },
            revision: Revision(3),
            ..Snapshot::default()
        };
        let value = serde_json::to_value(State::from(&snapshot)).unwrap();
        assert_eq!(value["version"], prysm_version::VERSION);
        assert_eq!(value["target"], "off");
        assert_eq!(value["revision"], 3);
        assert_eq!(
            value["session"],
            json!({
                "state": "stopped",
                "failure": { "kind": "adapter", "message": "device unplugged" },
            })
        );
        assert!(value.get("failure").is_none(), "only in the session");
        assert_eq!(
            value["settings"],
            serde_json::to_value(&snapshot.settings).unwrap()
        );
        assert!(value.get("input").is_none(), "absent while stopped");
        assert_eq!(value["device"], json!({}), "empty while stopped");
        assert!(
            value.get("calibration").is_none(),
            "absent while not calibrating"
        );
        let back: State = serde_json::from_value(value).unwrap();
        assert_eq!(back.session, (&snapshot.session).into());
        let idle = serde_json::to_value(State::from(&Snapshot::default())).unwrap();
        assert_eq!(
            idle["session"],
            json!({ "state": "stopped" }),
            "absent without one"
        );

        let running = Snapshot {
            session: SessionState::ScreenSync,
            input: Some(Input {
                fps: 29.9,
                receiving: true,
                ..Input::default()
            }),
            device: DeviceReport {
                issue: Some(OutputIssue::OtherSender),
                brightness: DeviceBrightness::from_percent(40),
                led_count: Some(120),
            },
            ..Snapshot::default()
        };
        let running = Snapshot {
            calibration: Some(CalibrationPattern {
                pattern: prysm_ports::Pattern::BluePair,
                uncorrected: true,
            }),
            ..running
        };
        let value = serde_json::to_value(State::from(&running)).unwrap();
        assert_eq!(
            value["calibration"],
            json!({ "pattern": "blue_pair", "uncorrected": true })
        );
        let back = serde_json::from_value::<State>(value.clone()).unwrap();
        assert_eq!(back.calibration, running.calibration);
        assert_eq!(back.device, running.device);
        assert_eq!(value["input"]["receiving"], true);
        assert_eq!(
            value["device"],
            json!({ "issue": "other_sender", "brightness_percent": 40, "led_count": 120 })
        );
        assert!((value["input"]["fps"].as_f64().unwrap() - 29.9).abs() < 1e-4);
        assert_eq!(value["session"], json!({ "state": "screen_sync" }));
    }

    /// Each session state on the wire: a failure only where the state
    /// carries one, a fault as words without its phase.
    #[test]
    fn sessions_carry_their_failure_and_a_fault_its_message() {
        let invalid = || Some(Error::from(ValidationError::new("output.leds", "too many")));
        let cases = [
            (SessionState::default(), json!({ "state": "stopped" })),
            (
                SessionState::Stopped { failure: invalid() },
                json!({ "state": "stopped", "failure": {
                    "kind": "invalid", "message": "too many", "field": "output.leds" } }),
            ),
            (SessionState::ScreenSync, json!({ "state": "screen_sync" })),
            (
                SessionState::Effects { failure: None },
                json!({ "state": "effects" }),
            ),
            (
                SessionState::Effects { failure: invalid() },
                json!({ "state": "effects", "failure": {
                    "kind": "invalid", "message": "too many", "field": "output.leds" } }),
            ),
            (
                SessionState::Stopped {
                    failure: Some(Error::Panicked("session thread")),
                },
                json!({ "state": "stopped", "failure": {
                    "kind": "internal", "message": "session thread panicked" } }),
            ),
            (
                SessionState::Stopped {
                    failure: Some(Error::Closed),
                },
                json!({ "state": "stopped", "failure": {
                    "kind": "internal", "message": "runtime has stopped" } }),
            ),
        ];
        for (state, expected) in cases {
            let session = Session::from(&state);
            assert_eq!(serde_json::to_value(&session).unwrap(), expected, "{state}");
            assert_eq!(
                serde_json::from_value::<Session>(expected).unwrap(),
                session
            );
        }
        for (phase, message) in [
            (SessionPhase::Running, "Lighting did not stop in time."),
            (
                SessionPhase::ClosingSource,
                "The video source did not close in time.",
            ),
            (
                SessionPhase::IdlingOutput,
                "The lights did not switch off in time.",
            ),
            (
                SessionPhase::Finishing,
                "Lighting did not finish stopping in time.",
            ),
            (
                SessionPhase::SourceUnreleased,
                "The video source did not confirm it was released.",
            ),
        ] {
            assert_eq!(
                serde_json::to_value(Session::from(&SessionState::Faulted(phase))).unwrap(),
                json!({ "state": "faulted", "message": message })
            );
        }
        let screen_sync_with_failure = json!({ "state": "screen_sync", "failure": {
            "kind": "internal", "message": "x" } });
        let read = serde_json::from_value::<Session>(screen_sync_with_failure).unwrap();
        assert_eq!(
            read,
            Session::ScreenSync,
            "screen sync has nowhere to keep one"
        );
    }

    /// Settings that cannot be loaded are the settings' problem: `invalid`
    /// with the store's whole cause chain, no field and no section.
    #[test]
    fn unusable_settings_are_invalid_with_their_cause_chain() {
        let stored = Error::adapter(
            "cannot parse settings file /data/settings.json",
            std::io::Error::other("expected value at line 1 column 3"),
        );
        let failure = Failure::from(&Error::UnusableSettings(std::sync::Arc::new(stored)));
        assert_eq!(
            serde_json::to_value(&failure).unwrap(),
            json!({
                "kind": "invalid",
                "message": "cannot parse settings file /data/settings.json: \
                            expected value at line 1 column 3",
            })
        );
    }

    #[test]
    fn failures_keep_field_current_revision_and_decode_reason() {
        let invalid = Failure::from(&Error::from(ValidationError::new(
            "output.leds",
            "too many",
        )));
        assert_eq!(
            invalid,
            Failure::Invalid {
                message: "too many".into(),
                field: Some("output.leds".into()),
            }
        );

        let stale = Failure::from(&Error::StaleRevision {
            expected: Revision(1),
            current: Revision(2),
        });
        assert_eq!(
            serde_json::to_value(&stale).unwrap(),
            json!({
                "kind": "stale_revision",
                "message": "settings revision 1 is stale; current is 2",
                "current": 2,
            })
        );

        let needs = Failure::from(&Error::NeedsScreenSync);
        assert_eq!(
            serde_json::to_value(&needs).unwrap(),
            json!({
                "kind": "needs_screen_sync",
                "message": "calibration patterns need screen sync: turn it on first",
            })
        );

        let source = serde_json::from_str::<u8>("\"x\"").unwrap_err();
        let decode = Failure::from(&Error::Decode {
            kind: "wled".into(),
            source: std::sync::Arc::new(source),
        });
        assert!(matches!(&decode, Failure::Invalid { field: None, .. }));
        assert!(
            decode
                .message()
                .starts_with("cannot decode `wled` settings: invalid type")
        );
    }

    /// The side an adapter failure came from is typed, for the portal to say
    /// which part of the path stopped; without one the field is absent.
    #[test]
    fn adapter_failures_carry_their_section() {
        let section = |failure: &Failure| match failure {
            Failure::Adapter { section, .. } => *section,
            other => panic!("not an adapter failure: {other:?}"),
        };
        let capture = Failure::from(&Error::other("no 640x400").in_section(Section::Capture));
        assert_eq!(section(&capture), Some(FailureSection::Capture));
        assert_eq!(
            serde_json::to_value(&capture).unwrap()["section"],
            "capture"
        );
        let output = Failure::from(&Error::other("gone").in_section(Section::Output));
        assert_eq!(section(&output), Some(FailureSection::Output));
        assert_eq!(
            LightingFailure::from(&Error::other("gone").in_section(Section::Output)),
            LightingFailure::Adapter {
                message: "gone".into(),
                section: Some(FailureSection::Output),
            }
        );
        let unknown = Failure::from(&Error::other("?"));
        assert_eq!(section(&unknown), None);
        assert!(
            serde_json::to_value(&unknown)
                .unwrap()
                .get("section")
                .is_none()
        );
        // Set once, where it happens: an outer tag does not overwrite it.
        let first = Error::other("x")
            .in_section(Section::Capture)
            .in_section(Section::Output);
        assert_eq!(first.section(), Some(Section::Capture));
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
        assert_eq!(
            failure,
            Failure::Adapter {
                message: "the sender cannot stream: cannot encode frame: \
                          hardware H.264 encoder cannot start"
                    .into(),
                section: None,
            }
        );
        let bare = Failure::from(&Error::other("no frames"));
        assert_eq!(bare.message(), "no frames");
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
