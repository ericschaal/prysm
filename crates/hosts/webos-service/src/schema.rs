//! The `luna` OpenAPI document: the values the TV app exchanges with
//! the TV service `luna://<APP_ID>.service` (feature `schema`, for the contract exporter).
//!
//! One document for both packages: `State` carries the installed package's
//! own fields in `Package`, tagged by `flavor`. Types from `prysm-webos-link` and `prysm-ports` are
//! described by schema-only mirrors, kept equal to their serde shape by tests,
//! except `StreamEncoding` and `OutputIssue`, which derive their own schemas
//! (`prysm-ports/schema`).
//!
//! | Method | Request | Reply |
//! | --- | --- | --- |
//! | `getState` | [`GetStateRequest`] | [`StateReply`], then one per change when subscribed |
//! | `setPreferences` | [`Preferences`] (complete) | [`StateReply`] |
//! | `setTarget` (full) | [`SetTargetRequest`] | [`StateReply`] |
//! | `setEnabled` (slim) | [`SetEnabledRequest`] | [`StateReply`] |
//! | `uninstall` | [`UninstallRequest`] | [`DoneReply`] |
//!
//! Any method can fail with [`ErrorReply`].
use crate::identity::APP_ID;
use crate::preferences::Preferences;
use crate::service::{
    DoneReply, ErrorKind, ErrorReply, GetStateRequest, SetEnabledRequest, SetTargetRequest,
    StateReply, UninstallRequest,
};
use crate::state::{Asked, FullWorker, Package, Power, Problem, SlimWorker, State};
use prysm_ports::{OutputIssue, StreamEncoding};
use serde::{Deserialize, Serialize};
pub use utoipa::openapi::OpenApi;
use utoipa::openapi::schema::{ObjectBuilder, Type};
use utoipa::{OpenApi as _, ToSchema};

/// The Luna API's own version, in the document's `info.version`; not the
/// product version, which only `VERSION` carries.
pub const API_VERSION: &str = "5";

// Mirrors `prysm_webos_link::Video`.
/// The TV's main video output. Open: an app reads a value it does not know
/// as `unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = Video)]
#[serde(rename_all = "snake_case")]
pub enum VideoSchema {
    Unknown,
    Unavailable,
    Available,
}

// Mirrors `prysm_ports::Fix`.
/// The person's next step. `try_again`: start again. `change_settings`:
/// change the settings (in the portal); a save that validates replaces them.
/// `set_up`: set up the service again: it lacks a permission it needs.
/// `unsupported`: nothing makes this work on this TV. `restart_tv`: restart
/// the TV: a worker that did not end may still hold the capture. Open: an
/// app reads a fix it does not know as `try_again`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = Fix)]
#[serde(rename_all = "snake_case")]
pub enum FixSchema {
    TryAgain,
    ChangeSettings,
    SetUp,
    Unsupported,
    RestartTv,
}

// Mirrors `prysm_ports::LightingTarget`.
/// What the user wants the LEDs to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = LightingTarget)]
#[serde(rename_all = "snake_case")]
pub enum LightingTargetSchema {
    Off,
    ScreenSync,
    Effects,
}

// Mirrors `prysm_webos_link::SlimLink`.
/// Where the slim worker's stream to the receiver stands, as `{ "state": … }`.
/// `starting`: no connection attempt yet. `sending`: connected, frames go
/// out in `encoding` (the one the receiver requested, set in the desktop
/// portal), `fps` per second (0 while none come). `refused`: connected, but
/// the TV refuses to share the video it shows, so no frames go out; it ends
/// when the video changes, not a failure. `paused`: the
/// receiver stopped the stream because its lighting is off; the TV capture
/// is released until a receiver takes a connection again, not a failure.
/// `waiting`: the receiver is unavailable (not lighting, unreachable, or its
/// name does not resolve); the worker keeps reconnecting, not a failure.
/// `encoder_failed`: the encoder the receiver requested could not start or
/// failed; the receiver is told, and another encoding chosen in its portal
/// may work. `version_differs`: Prysm speaks another capture stream version;
/// the older one needs updating, and the worker keeps reconnecting.
/// `receiver_failed`: Prysm's receiving source failed with this TV's stream
/// (a refusal, a keyframe that does not decode, no decoder), and Prysm shows
/// why; the worker released the capture and keeps reconnecting. `message`
/// says why, with the causes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = SlimLink)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum SlimLinkSchema {
    Starting {},
    Sending {
        encoding: StreamEncoding,
        /// Frames sent per second over about the last second, rounded.
        fps: u16,
    },
    Refused {},
    Paused {},
    Waiting {
        message: String,
    },
    EncoderFailed {
        message: String,
    },
    VersionDiffers {
        message: String,
    },
    ReceiverFailed {
        message: String,
    },
}

fn constant(value: bool) -> ObjectBuilder {
    ObjectBuilder::new()
        .schema_type(Type::Boolean)
        .enum_values(Some([value]))
}

/// The schema of LS2's `returnValue` on success.
pub(crate) fn always_true() -> ObjectBuilder {
    constant(true)
}

/// The schema of LS2's `returnValue` on failure.
pub(crate) fn always_false() -> ObjectBuilder {
    constant(false)
}

#[derive(utoipa::OpenApi)]
#[openapi(components(schemas(
    State,
    Package,
    FullWorker,
    SlimWorker,
    Power,
    Preferences,
    StreamEncoding,
    SlimLinkSchema,
    Problem,
    Asked,
    FixSchema,
    VideoSchema,
    LightingTargetSchema,
    OutputIssue,
    GetStateRequest,
    SetTargetRequest,
    SetEnabledRequest,
    UninstallRequest,
    StateReply,
    DoneReply,
    ErrorKind,
    ErrorReply,
)))]
struct Roots;

/// The `luna` document.
#[must_use]
pub fn document() -> OpenApi {
    let mut document = Roots::openapi();
    document.info.title = format!("Prysm TV service (luna://{APP_ID}.service)");
    document.info.version = API_VERSION.into();
    document
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_webos_link::{Fix, LightingTarget, SlimLink, Video};

    use prysm_testkit::assert_mirror_round_trips;
    use serde_json::{Value, json};

    #[test]
    fn mirrors_match_the_wire_types() {
        for video in [Video::Unknown, Video::Unavailable, Video::Available] {
            assert_mirror_round_trips::<Video, VideoSchema>(&video);
        }
        for target in LightingTarget::ALL {
            assert_mirror_round_trips::<LightingTarget, LightingTargetSchema>(&target);
        }
        for fix in Fix::ALL {
            assert_mirror_round_trips::<Fix, FixSchema>(&fix);
        }
        for encoding in StreamEncoding::ALL {
            assert_mirror_round_trips::<SlimLink, SlimLinkSchema>(&SlimLink::Sending {
                encoding,
                fps: 30,
            });
        }
        for link in [
            SlimLink::Starting {},
            SlimLink::Refused {},
            SlimLink::Paused {},
            SlimLink::Waiting {
                message: "x".into(),
            },
            SlimLink::EncoderFailed {
                message: "x".into(),
            },
            SlimLink::VersionDiffers {
                message: "x".into(),
            },
            SlimLink::ReceiverFailed {
                message: "x".into(),
            },
        ] {
            assert_mirror_round_trips::<SlimLink, SlimLinkSchema>(&link);
        }
    }

    /// The schema of `name`'s variant whose `tag` property is `value`.
    fn variant<'a>(schemas: &'a Value, name: &str, tag: &str, value: &str) -> &'a Value {
        schemas[name]["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|variant| variant["properties"][tag]["enum"] == json!([value]))
            .unwrap_or_else(|| panic!("{name} has no {tag} {value}"))
    }

    /// The properties `variant` requires, sorted.
    fn required(variant: &Value) -> Vec<&str> {
        let mut names: Vec<&str> = variant["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|name| name.as_str().unwrap())
            .collect();
        names.sort_unstable();
        names
    }

    #[test]
    fn document_describes_both_flavors_in_one_set_of_schemas() {
        let document = serde_json::to_value(document()).unwrap();
        assert_eq!(document["info"]["version"], json!("5"));
        let schemas = &document["components"]["schemas"];
        // The common fields, and the package's own tagged by `flavor`.
        let parts = schemas["State"]["allOf"].as_array().unwrap();
        assert_eq!(parts[0]["$ref"], json!("#/components/schemas/Package"));
        assert_eq!(
            parts[1]["properties"]["video"]["$ref"],
            json!("#/components/schemas/Video")
        );
        assert_eq!(
            required(&parts[1]),
            ["log", "power", "preferences", "video"]
        );
        let full = variant(schemas, "Package", "flavor", "full");
        assert_eq!(required(full), ["flavor", "target", "worker"]);
        assert_eq!(
            full["properties"]["worker"]["$ref"],
            json!("#/components/schemas/FullWorker")
        );
        let slim = variant(schemas, "Package", "flavor", "slim");
        assert_eq!(required(slim), ["enabled", "flavor", "worker"]);
        assert_eq!(
            slim["properties"]["worker"]["$ref"],
            json!("#/components/schemas/SlimWorker")
        );
        // Each package's running worker has only the fields it reports.
        let running = variant(schemas, "FullWorker", "phase", "running");
        assert_eq!(required(running), ["capturing", "phase"]);
        let mut fields: Vec<_> = running["properties"].as_object().unwrap().keys().collect();
        fields.sort_unstable();
        assert_eq!(
            fields,
            ["capturing", "fps", "lights", "phase", "portal", "problem"]
        );
        assert_eq!(
            running["properties"]["lights"]["$ref"],
            json!("#/components/schemas/OutputIssue")
        );
        let running = variant(schemas, "SlimWorker", "phase", "running");
        assert_eq!(required(running), ["capturing", "link", "phase"]);
        assert_eq!(
            running["properties"]["link"]["$ref"],
            json!("#/components/schemas/SlimLink")
        );
        // The slim link is one tag with only that state's fields.
        assert_eq!(
            required(variant(schemas, "SlimLink", "state", "sending")),
            ["encoding", "fps", "state"]
        );
        for (state, fields) in [
            ("starting", vec!["state"]),
            ("paused", vec!["state"]),
            ("waiting", vec!["message", "state"]),
            ("encoder_failed", vec!["message", "state"]),
            ("version_differs", vec!["message", "state"]),
            ("receiver_failed", vec!["message", "state"]),
        ] {
            assert_eq!(
                required(variant(schemas, "SlimLink", "state", state)),
                fields
            );
        }
        // Only asking always has a problem.
        for worker in ["FullWorker", "SlimWorker"] {
            assert_eq!(
                required(variant(schemas, worker, "phase", "stopping")),
                ["phase"]
            );
            let asking = variant(schemas, worker, "phase", "asking");
            assert_eq!(required(asking), ["phase", "problem"]);
            assert_eq!(
                asking["properties"]["problem"]["$ref"],
                json!("#/components/schemas/Problem")
            );
        }
        assert_eq!(required(&schemas["Problem"]), ["asked", "message"]);
        // What is asked is one tag, with the fix only for `ask`.
        assert_eq!(
            required(variant(schemas, "Asked", "step", "unanswered")),
            ["step"]
        );
        let ask = variant(schemas, "Asked", "step", "ask");
        assert_eq!(required(ask), ["fix", "step"]);
        assert_eq!(
            ask["properties"]["fix"]["$ref"],
            json!("#/components/schemas/Fix")
        );
        let preferences = &schemas["Preferences"];
        assert_eq!(preferences["additionalProperties"], json!(false));
        assert!(preferences.get("required").is_none_or(|r| r == &json!([])));
        // Picture settings are the receiver's or the portal's, not the TV's.
        assert_eq!(preferences["properties"].get("capture"), None);
        assert_eq!(schemas.get("SlimCapture"), None);
        let described = &preferences["properties"]["prysm_address"];
        let text = described["description"]
            .as_str()
            .or_else(|| described["oneOf"][0]["description"].as_str())
            .or_else(|| described["oneOf"][1]["description"].as_str())
            .unwrap_or_default();
        assert!(text.starts_with("Slim only"), "{described}");
        assert_eq!(
            schemas["StreamEncoding"]["enum"],
            json!(["h264_hardware", "h264_software", "raw"])
        );
        for name in [
            "GetStateRequest",
            "SetTargetRequest",
            "SetEnabledRequest",
            "UninstallRequest",
        ] {
            assert_eq!(
                schemas[name]["additionalProperties"],
                json!(false),
                "{name}"
            );
        }
        assert_eq!(
            schemas["ErrorKind"]["enum"],
            json!([
                "too_large",
                "unknown_method",
                "forbidden",
                "removing",
                "invalid_request",
                "wrong_package",
                "invalid_preferences",
                "save_failed",
                "internal"
            ])
        );
        assert_eq!(
            schemas["ErrorReply"]["properties"]["errorKind"]["$ref"],
            json!("#/components/schemas/ErrorKind")
        );
        let text = document.to_string();
        assert!(!text.contains(".slim"), "no per-flavor service names");
        assert_ne!(schemas["StateReply"], Value::Null);
    }
}
