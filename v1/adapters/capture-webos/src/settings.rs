use prysm_ports::ValidationError;
use serde::{Deserialize, Serialize};

/// `{ "kind": "webos", "width": 640, "height": 360, "fps": 30, "include_ui": true }`
///
/// LG's capture driver scales to `width`×`height` and paces at `fps`; all
/// fields are optional. `include_ui` blends on-screen menus into the capture
/// (LG dump mode 3) instead of video only (mode 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct WebosSettings {
    /// Even, 2 to 1920.
    #[cfg_attr(
        feature = "schema",
        schema(minimum = 2, maximum = 1920, multiple_of = 2, default = 640)
    )]
    pub width: u32,
    /// Even, 2 to 1080.
    #[cfg_attr(
        feature = "schema",
        schema(minimum = 2, maximum = 1080, multiple_of = 2, default = 360)
    )]
    pub height: u32,
    #[cfg_attr(feature = "schema", schema(minimum = 1, maximum = 60, default = 30))]
    pub fps: u32,
    /// Capture on-screen menus over the video and, when nothing is playing
    /// (such as the Home screen), the TV's own screens; false: video only.
    #[cfg_attr(feature = "schema", schema(default = true))]
    pub include_ui: bool,
}

impl Default for WebosSettings {
    fn default() -> Self {
        Self {
            width: 640,
            height: 360,
            fps: 30,
            include_ui: true,
        }
    }
}

impl WebosSettings {
    /// Rejects settings the driver can never negotiate, without touching it.
    ///
    /// # Errors
    /// The first invalid field, relative to the `capture` section.
    pub fn validate(&self) -> Result<(), ValidationError> {
        // NV12 needs even dimensions; the driver takes 16-bit sizes up to full HD.
        let even = |field: &str, value: u32, max: u32| {
            if (2..=max).contains(&value) && value.is_multiple_of(2) {
                Ok(())
            } else {
                Err(ValidationError::new(
                    field,
                    format!("must be an even number from 2 to {max}"),
                ))
            }
        };
        even("width", self.width, 1920)?;
        even("height", self.height, 1080)?;
        if !(1..=60).contains(&self.fps) {
            return Err(ValidationError::new("fps", "must be 1 to 60"));
        }
        Ok(())
    }

    /// The TV UI (graphics plane) capture size, used only while there is no
    /// video: each axis of the configured size clamped to 256–320 × 144–180.
    /// HAL's cost grows with the output size, and it fails below 256×144 on
    /// the measured TV. Axes are clamped independently: the plane is 16:9 and
    /// is scaled to fill the surface, exactly as the video tap fills a
    /// non-16:9 configured size, so edge sampling sees the same picture.
    pub(crate) fn ui_size(&self) -> (u16, u16) {
        let clamp = |value: u32, min: u16, max: u16| {
            u16::try_from(value.clamp(u32::from(min), u32::from(max))).unwrap_or(max)
        };
        (clamp(self.width, 256, 320), clamp(self.height, 144, 180))
    }

    /// The driver's options; only called on validated settings.
    pub(crate) fn options(&self) -> prysm_capture_webos_sys::Options {
        let narrow = |value: u32| u16::try_from(value).unwrap_or(u16::MAX);
        prysm_capture_webos_sys::Options {
            width: narrow(self.width),
            height: narrow(self.height),
            fps: u8::try_from(self.fps).unwrap_or(u8::MAX),
            include_ui: self.include_ui,
        }
    }
}

#[cfg(all(test, feature = "schema"))]
mod schema_tests {
    use super::*;
    use serde_json::{Value, json};
    use utoipa::PartialSchema;

    /// The schema lists exactly the serialized fields, requires none (serde
    /// defaults every field), carries the defaults and ranges `validate`
    /// enforces, and rejects unknown fields as serde does.
    #[test]
    fn schema_matches_serde_shape() {
        let schema = serde_json::to_value(WebosSettings::schema()).unwrap();
        let defaults = WebosSettings::default();
        let Value::Object(fields) = serde_json::to_value(defaults).unwrap() else {
            panic!("settings serialize as an object");
        };
        let properties = schema["properties"].as_object().unwrap();
        assert_eq!(
            fields.keys().collect::<Vec<_>>(),
            properties.keys().collect::<Vec<_>>()
        );
        for (key, value) in &fields {
            assert_eq!(&properties[key]["default"], value, "{key}");
        }
        assert!(schema.get("required").is_none_or(|r| r == &json!([])));
        assert_eq!(
            serde_json::from_value::<WebosSettings>(json!({})).unwrap(),
            defaults
        );
        assert_eq!(schema["additionalProperties"], json!(false));
        assert!(serde_json::from_value::<WebosSettings>(json!({ "unknown": 1 })).is_err());
        for (key, minimum, maximum) in [("width", 2, 1920), ("height", 2, 1080), ("fps", 1, 60)] {
            assert_eq!(properties[key]["minimum"], json!(minimum), "{key}");
            assert_eq!(properties[key]["maximum"], json!(maximum), "{key}");
            for value in [minimum, maximum] {
                let mut edge = defaults;
                *match key {
                    "width" => &mut edge.width,
                    "height" => &mut edge.height,
                    _ => &mut edge.fps,
                } = value;
                edge.validate().unwrap();
            }
        }
    }
}
