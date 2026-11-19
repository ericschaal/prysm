//! Server-known values the frontends use as they are, so they keep no copy:
//! generated into `@prysm/contracts/constants` as `export const` values.
use prysm_ports::{Style, StyleValues};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Each constant by its TypeScript name.
///
/// # Errors
/// A value that does not serialize, which is a bug.
pub fn serialized() -> serde_json::Result<Value> {
    Ok(json!({ "builtInStyles": shortest(&built_in_styles())? }))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_style_is_published_by_name() {
        let published = serialized().unwrap();
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
}
