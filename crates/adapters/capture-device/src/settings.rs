use prysm_ports::ValidationError;
use prysm_ports::capture_size::{
    MAX_FPS, default_fps, default_height, default_width, validate_capture_size,
};
use serde::{Deserialize, Serialize};

// `schema(...)` takes literals only: the rate's maximum below is spelled out.
const _: () = assert!(MAX_FPS == 120);

/// `{ "kind": "device", "device": "/dev/video0", "width": 640, "height": 360, "fps": 30 }`
///
/// `device` is a V4L2 device node on Linux and an AVFoundation device unique ID
/// on macOS. Size and rate default to 640×360 at 30 fps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DeviceSettings {
    /// V4L2 device node on Linux, AVFoundation device unique ID on macOS.
    #[cfg_attr(feature = "schema", schema(min_length = 1, example = "/dev/video0"))]
    pub device: String,
    #[serde(default = "default_width")]
    #[cfg_attr(feature = "schema", schema(minimum = 1, maximum = 3840, default = default_width))]
    pub width: u32,
    #[serde(default = "default_height")]
    #[cfg_attr(feature = "schema", schema(minimum = 1, maximum = 2160, default = default_height))]
    pub height: u32,
    #[serde(default = "default_fps")]
    #[cfg_attr(feature = "schema", schema(minimum = 1, maximum = 120, default = default_fps))]
    pub fps: u32,
}

impl DeviceSettings {
    #[must_use]
    pub fn new(device: impl Into<String>) -> Self {
        Self {
            device: device.into(),
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
        if self.device.trim().is_empty() || self.device.contains('\0') {
            return Err(ValidationError::new("device", "a device is required"));
        }
        validate_capture_size(self.width, self.height, self.fps)?;
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
    use prysm_testkit::assert_schema_matches_serde;
    use serde_json::json;

    #[test]
    fn schema_matches_serde_shape() {
        let schema = assert_schema_matches_serde(&DeviceSettings::new("/dev/video0"));
        assert_eq!(schema["required"], json!(["device"]));
        assert_eq!(schema["properties"]["width"]["default"], json!(640));
        assert_eq!(schema["properties"]["height"]["maximum"], json!(2160));
        assert_eq!(
            schema["properties"]["fps"]["maximum"],
            json!(prysm_ports::capture_size::MAX_FPS)
        );
    }
}
