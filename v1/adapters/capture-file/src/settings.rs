use prysm_core::CaptureResolution;
use prysm_ports::ValidationError;
use serde::{Deserialize, Serialize};

/// `{ "kind": "file", "path": "/home/me/clip.mkv", "width": 640, "height": 360, "fps": 30, "repeat": false }`
///
/// FFmpeg scales every frame to `width`×`height` and caps the rate at `fps`
/// (a slower file keeps its own rate). Size and rate default to 640×360 at
/// 30 fps. Playback runs once unless `repeat` is set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct FileSettings {
    /// Video file to play; any container and codec FFmpeg can decode.
    #[cfg_attr(
        feature = "schema",
        schema(min_length = 1, example = "/home/me/Videos/clip.mkv")
    )]
    pub path: String,
    #[serde(default = "default_width")]
    #[cfg_attr(feature = "schema", schema(minimum = 1, maximum = 3840, default = default_width))]
    pub width: u32,
    #[serde(default = "default_height")]
    #[cfg_attr(feature = "schema", schema(minimum = 1, maximum = 2160, default = default_height))]
    pub height: u32,
    /// Upper bound on the delivered frame rate.
    #[serde(default = "default_fps")]
    #[cfg_attr(feature = "schema", schema(minimum = 1, default = default_fps))]
    pub fps: u32,
    /// Loop the file forever instead of ending lighting at its end.
    #[serde(default)]
    #[cfg_attr(feature = "schema", schema(default = false))]
    pub repeat: bool,
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

impl FileSettings {
    #[must_use]
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            width: default_width(),
            height: default_height(),
            fps: default_fps(),
            repeat: false,
        }
    }

    /// Rejects settings that can never open, without touching the file.
    ///
    /// # Errors
    /// The first invalid field, relative to the `capture` section.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.path.trim().is_empty() || self.path.contains('\0') {
            return Err(ValidationError::new(
                "path",
                "a video file path is required",
            ));
        }
        if let Err(reason) = CaptureResolution::new(self.width, self.height) {
            // The width alone, with any valid height, says which is wrong.
            let field = if CaptureResolution::new(self.width, 1).is_err() {
                "width"
            } else {
                "height"
            };
            return Err(ValidationError::new(field, reason));
        }
        if self.fps == 0 {
            return Err(ValidationError::new("fps", "must be a positive frame rate"));
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
        let schema = same_shape(&FileSettings::new("/tmp/clip.mkv"));
        assert_eq!(schema["required"], json!(["path"]));
        assert_eq!(schema["properties"]["width"]["default"], json!(640));
        assert_eq!(schema["properties"]["height"]["maximum"], json!(2160));
        assert_eq!(schema["properties"]["fps"]["default"], json!(30));
        assert_eq!(FileSettings::name(), "FileSettings");
    }
}
