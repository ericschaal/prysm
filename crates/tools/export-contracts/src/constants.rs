//! Server-known values the frontends use as they are, so they keep no copy:
//! generated into `@prysm/contracts/constants` as `export const` values.
use prysm_ports::{Style, StyleValues};
use prysm_protocol::schema::OpenApi;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// Each constant by its TypeScript name, the adapter defaults read from
/// `documents`.
///
/// # Errors
/// A value that does not serialize, which is a bug.
pub fn serialized(documents: &BTreeMap<&str, OpenApi>) -> serde_json::Result<Value> {
    Ok(json!({
        "builtInStyles": shortest(&built_in_styles())?,
        "captureDefaults": capture_defaults(documents)?,
    }))
}

/// `value` read back from its text, so an `f32` keeps its shortest form
/// (0.8), where `json!` would widen it (0.800000011920929).
fn shortest(value: &BTreeMap<Style, StyleValues>) -> serde_json::Result<Value> {
    serde_json::from_str(&serde_json::to_string(value)?)
}

/// Each built-in style's values by name: the portal seeds a first Custom
/// style from the one that was chosen.
fn built_in_styles() -> BTreeMap<Style, StyleValues> {
    Style::BUILT_IN
        .into_iter()
        .filter_map(|style| Some((style, style.built_in()?)))
        .collect()
}

/// Each capture kind's fields that have a default, with it, by kind: the
/// `default` of each `CaptureSettings` variant's properties in the
/// documents that type them. The portal writes a new section in full from
/// them, so a later default changes only new setups.
fn capture_defaults(
    documents: &BTreeMap<&str, OpenApi>,
) -> serde_json::Result<BTreeMap<String, Map<String, Value>>> {
    let mut defaults = BTreeMap::new();
    for document in documents.values() {
        let document = serde_json::to_value(document)?;
        let schemas = &document["components"]["schemas"];
        let variants = schemas["CaptureSettings"]["oneOf"].as_array();
        for variant in variants.into_iter().flatten() {
            let Some(name) = variant["$ref"]
                .as_str()
                .and_then(|path| path.strip_prefix("#/components/schemas/"))
            else {
                continue;
            };
            let properties = &schemas[name]["properties"];
            let Some(kind) = properties["kind"]["enum"][0].as_str() else {
                continue;
            };
            let fields = properties
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(field, property)| {
                    Some((field.clone(), property.get("default")?.clone()))
                })
                .collect();
            defaults.insert(kind.to_owned(), fields);
        }
    }
    Ok(defaults)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_style_is_published_by_name() {
        let published = serialized(&crate::documents().unwrap()).unwrap();
        let styles = published["builtInStyles"].as_object().unwrap();
        assert_eq!(styles.len(), Style::BUILT_IN.len());
        for style in Style::BUILT_IN {
            let name = serde_json::to_value(style).unwrap();
            let values = &styles[name.as_str().unwrap()];
            let expected = style.built_in().unwrap();
            assert_eq!(
                serde_json::from_value::<StyleValues>(values.clone()).unwrap(),
                expected,
                "{name}"
            );
        }
        assert!(styles.get("custom").is_none());
        assert_eq!(styles["natural"]["smoothing_seconds"], json!(0.3));
    }

    /// Every capture kind a host registers, with the fields that have a
    /// default and only those.
    #[test]
    fn every_capture_kind_publishes_its_defaults() {
        let published = serialized(&crate::documents().unwrap()).unwrap();
        assert_eq!(
            published["captureDefaults"],
            json!({
                "device": { "width": 640, "height": 360, "fps": 30 },
                "file": { "width": 640, "height": 360, "fps": 30, "repeat": false },
                "remote": {
                    "width": 320,
                    "height": 180,
                    "fps": 30,
                    "include_ui": true,
                    "encoding": "h264_hardware",
                },
                "webos": { "width": 640, "height": 360, "fps": 30, "include_ui": true },
            })
        );
    }
}
