//! The `luna` OpenAPI document: the values the TV app exchanges with
//! the TV service `luna://<APP_ID>.service` (feature `schema`, for the contract exporter).
//!
//! One document for both packages: `State` carries the installed package's
//! own fields in `Package`, tagged by `flavor`. The wire types derive their schemas.
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
    DoneReply, ErrorReply, GetStateRequest, SetEnabledRequest, SetTargetRequest, StateReply,
    UninstallRequest,
};
pub use prysm_ports::LightingTarget as LightingTargetSchema;
pub use prysm_webos_link::{Fix as FixSchema, SlimLink as SlimLinkSchema, Video as VideoSchema};
use utoipa::OpenApi as _;
pub use utoipa::openapi::OpenApi;
use utoipa::openapi::schema::{ObjectBuilder, Type};

/// The Luna API's own version, in the document's `info.version`; not the
/// product version, which only `VERSION` carries.
pub const API_VERSION: &str = "5";

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
    GetStateRequest,
    SetTargetRequest,
    SetEnabledRequest,
    UninstallRequest,
    Preferences,
    StateReply,
    DoneReply,
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
    use serde_json::{Value, json};

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
        assert_eq!(
            required(variant(schemas, "SlimLink", "state", "sending")),
            ["encoding", "fps", "state"]
        );
        for (state, fields) in [
            ("starting", vec!["state"]),
            ("paused", vec!["state"]),
            ("waiting", vec!["message", "state"]),
            ("capture_waiting", vec!["message", "state"]),
            ("encoder_failed", vec!["message", "state"]),
            ("version_differs", vec!["message", "state"]),
            ("receiver_failed", vec!["message", "state"]),
        ] {
            assert_eq!(
                required(variant(schemas, "SlimLink", "state", state)),
                fields
            );
        }
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
