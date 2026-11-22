//! The `luna` OpenAPI document: the values the TV app exchanges with
//! the TV service `luna://<APP_ID>.service` (feature `schema`, for the contract exporter).
//!
//! One document for both packages: `State` carries the installed package's
//! own fields in `Package`, tagged by `flavor`. Types from `prysm-webos-link` and `prysm-ports` are
//! described by schema-only mirrors, kept equal to their serde shape by tests.
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
    ErrorKind, GetStateRequest, SetEnabledRequest, SetTargetRequest, UninstallRequest,
};
use crate::state::{FullWorker, Package, Power, SlimWorker, State};
use serde::{Deserialize, Serialize};
pub use utoipa::openapi::OpenApi;
use utoipa::openapi::schema::{ObjectBuilder, Type};
use utoipa::{OpenApi as _, ToSchema};

/// The Luna API's own version, in the document's `info.version`; not the
/// product version, which only `VERSION` carries.
pub const API_VERSION: &str = "4";

// Mirrors `prysm_webos_link::Video`.
/// The TV's main video output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = Video)]
#[serde(rename_all = "snake_case")]
pub enum VideoSchema {
    Unknown,
    Unavailable,
    Available,
}

// Mirrors `prysm_webos_link::FailureKind`.
/// What a failure is, so the app can offer the matching fix. `encoder`
/// (slim): the encoder the receiver requested could not start or failed;
/// the receiver is told, and another encoding chosen in its portal may work.
/// `receiver` (slim): the receiver is unavailable (not lighting, unreachable,
/// or its name does not resolve); the worker keeps reconnecting. `version`
/// (slim): Prysm speaks another capture stream version; the older one needs
/// updating, and the worker keeps reconnecting. `replaced` (slim): another
/// TV took over the stream at Prysm; this one stopped sending until the
/// worker restarts (capture stopped and started, the TV waking, or a new
/// Prysm address). `receiver_failed` (slim): Prysm's receiving source failed
/// with this TV's stream (a refusal, a keyframe that does not decode, no
/// decoder), and Prysm shows why; the worker released the capture and keeps
/// reconnecting. `preferences`: the TV's saved preferences could not be
/// read; the service runs with the defaults and keeps the file until the app
/// saves preferences. `other`: anything else. Open: an app reads a kind it
/// does not know as `other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = FailureKind)]
#[serde(rename_all = "snake_case")]
pub enum FailureKindSchema {
    Encoder,
    Receiver,
    Version,
    Replaced,
    ReceiverFailed,
    Preferences,
    Other,
}

// Mirrors `prysm_webos_link::WorkerFailure`.
/// A failure: what it is and its message with the causes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = WorkerFailure)]
#[serde(deny_unknown_fields)]
pub struct WorkerFailureSchema {
    pub kind: FailureKindSchema,
    /// What failed, with its cause chain (cut to 1 KiB).
    pub message: String,
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

// Mirrors `prysm_webos_link::SlimEncoding`.
/// How the slim package encodes the frames it sends: the encoding its
/// receiver requested (set in the desktop portal). Exactly this encoder is
/// used; one that cannot start or fails is a failure, never a switch to
/// another. `h264_hardware`: the TV's hardware H.264 encoder (about
/// 2 Mbit/s); `h264_software`: OpenH264 on the TV's CPU (about 2 Mbit/s);
/// `raw`: uncompressed NV12 (320x180 at 30 fps is about 21 Mbit/s).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = SlimEncoding)]
#[serde(rename_all = "snake_case")]
pub enum SlimEncodingSchema {
    H264Hardware,
    H264Software,
    Raw,
}

// Mirrors `prysm_webos_link::SlimLink`.
/// Where the slim worker's stream to the receiver stands, as `{ "state": … }`.
/// `starting`: no connection attempt yet. `sending`: connected, frames go
/// out in `encoding`, `fps` per second (0 while none come). `paused`: the
/// receiver stopped the stream because its lighting is off; the TV capture
/// is released until a receiver takes a connection again, not a failure.
/// `waiting`: the receiver is unavailable (`failure.kind` `receiver`).
/// `failed`: the encoder, version, replaced or receiver-failed failure. The
/// state's own `failure` is the one to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = SlimLink)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum SlimLinkSchema {
    Starting {},
    Sending {
        encoding: SlimEncodingSchema,
        /// Frames sent per second over about the last second, rounded.
        fps: u16,
    },
    Paused {},
    Waiting {
        failure: WorkerFailureSchema,
    },
    Failed {
        failure: WorkerFailureSchema,
    },
}

fn constant(value: bool) -> ObjectBuilder {
    ObjectBuilder::new()
        .schema_type(Type::Boolean)
        .enum_values(Some([value]))
}

fn always_true() -> ObjectBuilder {
    constant(true)
}

fn always_false() -> ObjectBuilder {
    constant(false)
}

/// A successful state reply: [`State`] with LS2's `returnValue`.
#[derive(ToSchema)]
#[allow(dead_code, non_snake_case)]
pub struct StateReply {
    #[serde(flatten)]
    state: State,
    #[schema(schema_with = always_true)]
    returnValue: bool,
    /// On `getState` and subscription updates: whether the caller is subscribed.
    #[schema(nullable = false)]
    subscribed: Option<bool>,
}

/// The `uninstall` reply.
#[derive(ToSchema)]
#[allow(dead_code, non_snake_case)]
pub struct DoneReply {
    #[schema(schema_with = always_true)]
    returnValue: bool,
}

/// Any failed call. LS2's own replies (for example for a method that is not
/// registered) carry no `errorKind`; an app reads that as an unknown kind.
#[derive(ToSchema)]
#[allow(dead_code, non_snake_case)]
pub struct ErrorReply {
    #[schema(schema_with = always_false)]
    returnValue: bool,
    errorKind: ErrorKind,
    /// What failed, for people.
    errorText: String,
}

#[derive(utoipa::OpenApi)]
#[openapi(components(schemas(
    State,
    Package,
    FullWorker,
    SlimWorker,
    Power,
    Preferences,
    SlimEncodingSchema,
    SlimLinkSchema,
    FailureKindSchema,
    WorkerFailureSchema,
    VideoSchema,
    LightingTargetSchema,
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
    use prysm_webos_link::{
        FailureKind, LightingTarget, SlimEncoding, SlimLink, Video, WorkerFailure,
    };

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
        for kind in [
            FailureKind::Encoder,
            FailureKind::Receiver,
            FailureKind::Version,
            FailureKind::Replaced,
            FailureKind::ReceiverFailed,
            FailureKind::Preferences,
            FailureKind::Other,
        ] {
            assert_mirror_round_trips::<FailureKind, FailureKindSchema>(&kind);
            assert_mirror_round_trips::<WorkerFailure, WorkerFailureSchema>(&WorkerFailure::new(
                kind, "x",
            ));
        }
        for encoding in [
            SlimEncoding::H264Hardware,
            SlimEncoding::H264Software,
            SlimEncoding::Raw,
        ] {
            assert_mirror_round_trips::<SlimEncoding, SlimEncodingSchema>(&encoding);
            assert_mirror_round_trips::<SlimLink, SlimLinkSchema>(&SlimLink::Sending {
                encoding,
                fps: 30,
            });
        }
        for link in [
            SlimLink::Starting {},
            SlimLink::Paused {},
            SlimLink::Waiting {
                failure: WorkerFailure::new(FailureKind::Receiver, "x"),
            },
            SlimLink::Failed {
                failure: WorkerFailure::new(FailureKind::Encoder, "x"),
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
        assert_eq!(document["info"]["version"], json!("4"));
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
        assert_eq!(fields, ["capturing", "fps", "phase", "portal"]);
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
            ("waiting", vec!["failure", "state"]),
            ("failed", vec!["failure", "state"]),
        ] {
            assert_eq!(
                required(variant(schemas, "SlimLink", "state", state)),
                fields
            );
        }
        assert_eq!(
            required(variant(schemas, "SlimWorker", "phase", "stopping")),
            ["phase"]
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
            schemas["SlimEncoding"]["enum"],
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
