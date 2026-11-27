//! The receiver's settings: what it asks its sender for, the stream's shape
//! and encoding. Its request message carries them to the sender.
use crate::wire::{MAX_FPS, MAX_HEIGHT, MAX_WIDTH};
use prysm_h264::{Backend, EncoderSettings};
use prysm_ports::{StreamEncoding, ValidationError};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The settings of kind `"remote"`, and the stream a receiver asks for:
/// `{ "kind": "remote", "width": 320, "height": 180, "fps": 30,
/// "include_ui": true, "encoding": "h264_hardware" }`, every field optional.
/// The sender captures at this size, rate and menu blending and encodes with
/// exactly this encoding. [`validate`](Self::validate) keeps the protocol's
/// bounds; what a sender's capture and encoder deliver is in its
/// [`Offer`](crate::Offer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "schema", schema(as = RemoteReceiverSettings))]
pub struct RemoteSettings {
    /// Requested picture width: even, 2 to 1920; the sender's offer says
    /// what its capture and encoder deliver.
    #[cfg_attr(
        feature = "schema",
        schema(minimum = 2, maximum = 1920, multiple_of = 2, default = 320)
    )]
    pub width: u32,
    /// Requested picture height: even, 2 to 1080; the sender's offer says
    /// what its capture and encoder deliver.
    #[cfg_attr(
        feature = "schema",
        schema(minimum = 2, maximum = 1080, multiple_of = 2, default = 180)
    )]
    pub height: u32,
    /// Requested frames per second.
    #[cfg_attr(feature = "schema", schema(minimum = 1, maximum = 60, default = 30))]
    pub fps: u32,
    /// Capture on-screen menus over the video and, when nothing is playing
    /// (such as the Home screen), the TV's own screens; false: video only.
    #[cfg_attr(feature = "schema", schema(default = true))]
    pub include_ui: bool,
    /// How the sender encodes the stream. It uses exactly this encoder; if
    /// that fails, lighting stops with the sender's reason.
    #[cfg_attr(feature = "schema", schema(default = "h264_hardware"))]
    pub encoding: StreamEncoding,
}

impl Default for RemoteSettings {
    fn default() -> Self {
        Self {
            width: 320,
            height: 180,
            fps: 30,
            include_ui: true,
            encoding: StreamEncoding::default(),
        }
    }
}

impl RemoteSettings {
    /// Rejects what the protocol never carries, without asking a sender.
    ///
    /// # Errors
    /// The first invalid field, relative to the `capture` section.
    pub fn validate(&self) -> Result<(), ValidationError> {
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
        even("width", self.width, MAX_WIDTH)?;
        even("height", self.height, MAX_HEIGHT)?;
        if !(1..=MAX_FPS).contains(&self.fps) {
            return Err(ValidationError::new(
                "fps",
                format!("must be 1 to {MAX_FPS}"),
            ));
        }
        Ok(())
    }

    /// The encoder this request names; `None` for raw.
    #[must_use]
    pub fn encoder(&self) -> Option<EncoderSettings> {
        Backend::of(self.encoding).map(|backend| EncoderSettings::new(backend, self.fps))
    }
}

impl fmt::Display for RemoteSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}x{} at {} fps{} as {}",
            self.width,
            self.height,
            self.fps,
            if self.include_ui { " with menus" } else { "" },
            self.encoding
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate_and_limits_are_enforced() {
        let request = RemoteSettings::default();
        request.validate().unwrap();
        assert_eq!(
            request.to_string(),
            "320x180 at 30 fps with menus as h264_hardware"
        );
        for (bad, field) in [
            (
                RemoteSettings {
                    width: 321,
                    ..request
                },
                "width",
            ),
            (
                RemoteSettings {
                    width: 1922,
                    ..request
                },
                "width",
            ),
            (
                RemoteSettings {
                    height: 0,
                    ..request
                },
                "height",
            ),
            (RemoteSettings { fps: 61, ..request }, "fps"),
        ] {
            assert_eq!(bad.validate().unwrap_err().field, field, "{bad:?}");
        }
        RemoteSettings {
            width: 8,
            height: 2,
            ..request
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn encodings_name_their_encoder() {
        let request = |encoding| RemoteSettings {
            encoding,
            ..RemoteSettings::default()
        };
        assert_eq!(
            request(StreamEncoding::H264Hardware).encoder(),
            Some(EncoderSettings::new(Backend::Hardware, 30))
        );
        assert_eq!(
            request(StreamEncoding::H264Software).encoder(),
            Some(EncoderSettings::new(Backend::Software, 30))
        );
        assert_eq!(request(StreamEncoding::Raw).encoder(), None);
    }
}

#[cfg(all(test, feature = "schema"))]
mod schema_tests {
    use super::*;
    use prysm_testkit::assert_schema_matches_serde;
    use serde_json::json;

    #[test]
    fn schema_matches_serde_shape() {
        let settings = RemoteSettings::default();
        let schema = assert_schema_matches_serde(&settings);
        assert!(
            schema
                .get("required")
                .is_none_or(|required| required == &json!([]))
        );
        assert!(schema["properties"].get("listen").is_none());
        for (field, default) in [
            ("width", json!(320)),
            ("height", json!(180)),
            ("fps", json!(30)),
            ("include_ui", json!(true)),
            ("encoding", json!("h264_hardware")),
        ] {
            assert_eq!(schema["properties"][field]["default"], default, "{field}");
            assert_eq!(serde_json::to_value(settings).unwrap()[field], default);
        }
        for (field, maximum) in [
            ("width", crate::wire::MAX_WIDTH),
            ("height", crate::wire::MAX_HEIGHT),
            ("fps", crate::wire::MAX_FPS),
        ] {
            assert_eq!(
                schema["properties"][field]["maximum"],
                json!(maximum),
                "{field}"
            );
        }
    }
}
