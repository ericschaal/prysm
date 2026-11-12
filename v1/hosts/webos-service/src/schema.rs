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

// Mirrors `prysm_webos_link::SlimCapture`.
/// Capture size, rate and menu blending of the slim package; the same rules
/// as the `webos` capture settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = SlimCapture)]
#[serde(default, deny_unknown_fields)]
pub struct SlimCaptureSchema {
    /// Even, 2 to 1920.
    #[schema(minimum = 2, maximum = 1920, multiple_of = 2, default = 320)]
    width: u32,
    /// Even, 2 to 1080.
    #[schema(minimum = 2, maximum = 1080, multiple_of = 2, default = 180)]
    height: u32,
    #[schema(minimum = 1, maximum = 60, default = 30)]
    fps: u32,
    /// Blend TV menus into the capture.
    #[schema(default = true)]
    include_ui: bool,
}

impl Default for SlimCaptureSchema {
    fn default() -> Self {
        Self {
            width: 320,
            height: 180,
            fps: 30,
            include_ui: true,
        }
    }
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
    SlimCaptureSchema,
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
    use prysm_webos_link::{LightingTarget, SlimCapture, Video};
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
        same_shape::<SlimCapture, SlimCaptureSchema>(&SlimCapture::default());
        same_shape::<SlimCapture, SlimCaptureSchema>(&SlimCapture {
            width: 1920,
            height: 1080,
            fps: 60,
            include_ui: false,
        });
        assert_eq!(
            serde_json::to_value(SlimCaptureSchema::default()).unwrap(),
            serde_json::to_value(SlimCapture::default()).unwrap()
        );
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
        for field in ["instance", "receiver", "capture"] {
            let described = &preferences["properties"][field];
            let text = described["description"]
                .as_str()
                .or_else(|| described["oneOf"][1]["description"].as_str())
                .unwrap_or_default();
            assert!(text.starts_with("Slim only"), "{field}: {described}");
        }
        assert_eq!(
            schemas["SlimCapture"]["properties"]["fps"]["maximum"],
            json!(60)
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
