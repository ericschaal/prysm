//! The `luna` OpenAPI document: the values the TV app exchanges with
//! `luna://io.prysm.tv.service` (feature `schema`, for the contract exporter).
//!
//! One document for both packages; fields that only one flavor uses say so
//! in their description. Types from `prysm-webos-link` and `prysm-ports` are
//! described by schema-only mirrors, kept equal to their serde shape by tests.
//!
//! | Method | Request | Reply |
//! | --- | --- | --- |
//! | `getState` | [`GetStateRequest`] | [`StateReply`], then one per change when subscribed |
//! | `setPreferences` | [`Preferences`] (complete) | [`StateReply`] |
//! | `setEnabled` | [`SetEnabledRequest`] | [`StateReply`] |
//! | `uninstall` | [`UninstallRequest`] | [`DoneReply`] |
//!
//! Any method can fail with [`ErrorReply`].
use crate::preferences::Preferences;
use crate::service::{GetStateRequest, SetEnabledRequest, UninstallRequest};
use crate::state::{Flavor, Phase, Power, State};
use serde::{Deserialize, Serialize};
pub use utoipa::openapi::OpenApi;
use utoipa::openapi::schema::{ObjectBuilder, Type};
use utoipa::{OpenApi as _, ToSchema};

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
/// or its name does not resolve); the worker keeps reconnecting. `other`: anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = FailureKind)]
#[serde(rename_all = "snake_case")]
pub enum FailureKindSchema {
    Encoder,
    Receiver,
    Other,
}

// Mirrors `prysm_ports::LightingTarget`.
/// What the user wants the LEDs to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = LightingTarget)]
#[serde(rename_all = "snake_case")]
pub enum LightingTargetSchema {
    Off,
    Processed,
    DeviceNative,
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

/// Any failed call.
#[derive(ToSchema)]
#[allow(dead_code, non_snake_case)]
pub struct ErrorReply {
    #[schema(schema_with = always_false)]
    returnValue: bool,
    errorText: String,
}

#[derive(utoipa::OpenApi)]
#[openapi(components(schemas(
    State,
    Flavor,
    Power,
    Phase,
    Preferences,
    SlimEncodingSchema,
    FailureKindSchema,
    VideoSchema,
    LightingTargetSchema,
    GetStateRequest,
    SetEnabledRequest,
    UninstallRequest,
    StateReply,
    DoneReply,
    ErrorReply,
)))]
struct Roots;

/// The `luna` document.
#[must_use]
pub fn document() -> OpenApi {
    let mut document = Roots::openapi();
    document.info.title = "Prysm TV service (luna://io.prysm.tv.service)".into();
    document.info.version = env!("CARGO_PKG_VERSION").into();
    document
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_webos_link::{FailureKind, LightingTarget, SlimEncoding, Video};
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};
    use std::fmt;

    /// Each mirror reads what the real type writes, and vice versa.
    fn same_shape<P, M>(value: &P)
    where
        P: Serialize + DeserializeOwned + PartialEq + fmt::Debug,
        M: Serialize + DeserializeOwned,
    {
        let json = serde_json::to_value(value).unwrap();
        let mirror: M = serde_json::from_value(json.clone()).unwrap();
        let back = serde_json::to_value(&mirror).unwrap();
        assert_eq!(back, json);
        assert_eq!(&serde_json::from_value::<P>(back).unwrap(), value);
    }

    #[test]
    fn mirrors_match_the_wire_types() {
        for video in [Video::Unknown, Video::Unavailable, Video::Available] {
            same_shape::<Video, VideoSchema>(&video);
        }
        for target in [
            LightingTarget::Off,
            LightingTarget::Processed,
            LightingTarget::DeviceNative,
        ] {
            same_shape::<LightingTarget, LightingTargetSchema>(&target);
        }
        for kind in [
            FailureKind::Encoder,
            FailureKind::Receiver,
            FailureKind::Other,
        ] {
            same_shape::<FailureKind, FailureKindSchema>(&kind);
        }
        for encoding in [
            SlimEncoding::H264Hardware,
            SlimEncoding::H264Software,
            SlimEncoding::Raw,
        ] {
            same_shape::<SlimEncoding, SlimEncodingSchema>(&encoding);
        }
    }

    #[test]
    fn document_describes_both_flavors_in_one_set_of_schemas() {
        let document = serde_json::to_value(document()).unwrap();
        let schemas = &document["components"]["schemas"];
        assert_eq!(schemas["Flavor"]["enum"], json!(["full", "slim"]));
        assert_eq!(
            schemas["State"]["properties"]["video"]["$ref"],
            json!("#/components/schemas/Video")
        );
        let preferences = &schemas["Preferences"];
        assert_eq!(preferences["additionalProperties"], json!(false));
        assert!(preferences.get("required").is_none_or(|r| r == &json!([])));
        // Picture settings are the receiver's or the portal's, not the TV's.
        assert_eq!(preferences["properties"].get("capture"), None);
        assert_eq!(schemas.get("SlimCapture"), None);
        for field in ["instance", "receiver"] {
            let described = &preferences["properties"][field];
            let text = described["description"]
                .as_str()
                .or_else(|| described["oneOf"][0]["description"].as_str())
                .or_else(|| described["oneOf"][1]["description"].as_str())
                .unwrap_or_default();
            assert!(text.starts_with("Slim only"), "{field}: {described}");
        }
        assert_eq!(
            schemas["SlimEncoding"]["enum"],
            json!(["h264_hardware", "h264_software", "raw"])
        );
        for name in ["GetStateRequest", "SetEnabledRequest", "UninstallRequest"] {
            assert_eq!(
                schemas[name]["additionalProperties"],
                json!(false),
                "{name}"
            );
        }
        let text = document.to_string();
        assert!(!text.contains(".slim"), "no per-flavor service names");
        assert_ne!(schemas["StateReply"], Value::Null);
    }
}
