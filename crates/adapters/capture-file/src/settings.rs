use prysm_ports::ValidationError;
use prysm_ports::capture_size::{
    MAX_FPS, default_fps, default_height, default_width, validate_capture_size,
};
use serde::{Deserialize, Serialize};

// `schema(...)` takes literals only: the rate's maximum below is spelled out.
const _: () = assert!(MAX_FPS == 120);

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
    #[cfg_attr(feature = "schema", schema(minimum = 1, maximum = 120, default = default_fps))]
    pub fps: u32,
    /// Loop the file forever instead of ending lighting at its end.
    #[serde(default)]
    #[cfg_attr(feature = "schema", schema(default = false))]
    pub repeat: bool,
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
        validate_capture_size(self.width, self.height, self.fps)
    }
}

#[cfg(all(test, feature = "schema"))]
mod schema_tests {
    use super::*;
    use prysm_testkit::assert_schema_matches_serde;
    use serde_json::json;
    use utoipa::ToSchema;

    #[test]
    fn schema_matches_serde_shape() {
        let schema = assert_schema_matches_serde(&FileSettings::new("/tmp/clip.mkv"));
        assert_eq!(schema["required"], json!(["path"]));
        assert_eq!(schema["properties"]["width"]["default"], json!(640));
        assert_eq!(schema["properties"]["height"]["maximum"], json!(2160));
        assert_eq!(schema["properties"]["fps"]["default"], json!(30));
        assert_eq!(
            schema["properties"]["fps"]["maximum"],
            json!(prysm_ports::capture_size::MAX_FPS)
        );
        assert_eq!(FileSettings::name(), "FileSettings");
    }
}
