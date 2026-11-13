use prysm_core::CaptureResolution;
use prysm_ports::ValidationError;
use serde::{Deserialize, Serialize};

/// `{ "kind": "device", "path": "/dev/video0", "width": 640, "height": 360, "fps": 30 }`
///
/// `path` is a V4L2 device node on Linux and an AVFoundation device unique ID
/// on macOS. Size and rate default to 640×360 at 30 fps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DeviceSettings {
    /// V4L2 device node on Linux, AVFoundation device unique ID on macOS.
    #[cfg_attr(feature = "schema", schema(min_length = 1, example = "/dev/video0"))]
    pub path: String,
    #[serde(default = "default_width")]
    #[cfg_attr(feature = "schema", schema(minimum = 1, maximum = 3840, default = default_width))]
    pub width: u32,
    #[serde(default = "default_height")]
    #[cfg_attr(feature = "schema", schema(minimum = 1, maximum = 2160, default = default_height))]
    pub height: u32,
    #[serde(default = "default_fps")]
    #[cfg_attr(feature = "schema", schema(minimum = 1, default = default_fps))]
    pub fps: u32,
}

fn default_width() -> u32 {
    CaptureResolution::default().dimensions()[0]
}

fn default_height() -> u32 {
    CaptureResolution::default().dimensions()[1]
}

const fn default_fps() -> u32 {
    30
}

impl DeviceSettings {
    #[must_use]
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            width: default_width(),
            height: default_height(),
            fps: default_fps(),
        }
    }

    /// Rejects settings that can never open, without touching hardware.
    ///
    /// # Errors
    /// The first invalid field, relative to the `capture` section.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.path.trim().is_empty() || self.path.contains('\0') {
            return Err(ValidationError::new(
                "path",
                "a device path or identifier is required",
            ));
        }
        if let Err(reason) = CaptureResolution::new(self.width, self.height) {
            let field = if self.width == 0 || self.width > 3840 {
                "width"
            } else {
                "height"
            };
            return Err(ValidationError::new(field, reason));
        }
        // AVFoundation expresses frame durations with an i32 timescale.
        if self.fps == 0 || (cfg!(target_os = "macos") && i32::try_from(self.fps).is_err()) {
            return Err(ValidationError::new("fps", "must be a positive frame rate"));
        }
        if !cfg!(any(target_os = "linux", target_os = "macos")) {
            return Err(ValidationError::new(
                "kind",
                "video device capture is unsupported on this platform",
            ));
        }
        Ok(())
    }
}
#[cfg(all(test, feature = "schema"))]
mod schema_tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::{Map, Value, json};
    use std::collections::BTreeSet;
    use utoipa::ToSchema;

    /// The schema lists exactly the serialized fields, requires exactly those
    /// serde cannot default, and rejects unknown fields as serde does.
    fn same_shape<T: ToSchema + Serialize + DeserializeOwned>(full: &T) -> Value {
        let schema = serde_json::to_value(T::schema()).unwrap();
        let properties = schema["properties"].as_object().unwrap();
        let required: Vec<String> =
            serde_json::from_value(schema["required"].clone()).unwrap_or_default();
        let Value::Object(fields) = serde_json::to_value(full).unwrap() else {
            panic!("settings serialize as an object");
        };
        let keys = |map: &Map<String, Value>| map.keys().cloned().collect::<BTreeSet<_>>();
        assert_eq!(keys(&fields), keys(properties));
        let minimal: Map<String, Value> = fields
            .iter()
            .filter(|(key, _)| required.contains(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        serde_json::from_value::<T>(minimal.clone().into()).unwrap();
        for key in &required {
            let mut missing = minimal.clone();
            missing.remove(key);
            assert!(
                serde_json::from_value::<T>(missing.into()).is_err(),
                "{key}"
            );
        }
        assert_eq!(schema["additionalProperties"], json!(false));
        let mut unknown = fields;
        unknown.insert("unknown".into(), json!(1));
        assert!(serde_json::from_value::<T>(unknown.into()).is_err());
        schema
    }

    #[test]
    fn schema_matches_serde_shape() {
        let schema = same_shape(&DeviceSettings::new("/dev/video0"));
        assert_eq!(schema["required"], json!(["path"]));
        assert_eq!(schema["properties"]["width"]["default"], json!(640));
        assert_eq!(schema["properties"]["height"]["maximum"], json!(2160));
    }
}
