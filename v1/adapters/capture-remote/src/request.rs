//! What a receiver asks its sender for: the stream's shape and encoding. The
//! receiver's settings hold it; the hello reply carries it to the sender.
use prysm_h264::{Backend, EncoderSettings};
use prysm_ports::ValidationError;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// How the sender encodes the frames it sends. Exactly this encoder is used:
/// one that cannot start or fails ends the stream with a refusal, never a
/// switch to another.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum StreamEncoding {
    /// The sender's hardware H.264 encoder (the webOS TV's; about 2 Mbit/s).
    #[default]
    H264Hardware,
    /// OpenH264 on the sender's CPU (about 2 Mbit/s).
    H264Software,
    /// Uncompressed pixels (320x180 NV12 at 30 fps is about 21 Mbit/s).
    Raw,
}

impl StreamEncoding {
    const ALL: [Self; 3] = [Self::H264Hardware, Self::H264Software, Self::Raw];

    /// The serialized name, such as `h264_hardware`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::H264Hardware => "h264_hardware",
            Self::H264Software => "h264_software",
            Self::Raw => "raw",
        }
    }

    /// The H.264 encoder backend; `None` for raw.
    #[must_use]
    pub const fn backend(self) -> Option<Backend> {
        match self {
            Self::H264Hardware => Some(Backend::Hardware),
            Self::H264Software => Some(Backend::Software),
            Self::Raw => None,
        }
    }

    pub(crate) const fn code(self) -> u8 {
        match self {
            Self::Raw => 0,
            Self::H264Hardware => 1,
            Self::H264Software => 2,
        }
    }

    pub(crate) fn from_code(code: u8) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|encoding| encoding.code() == code)
    }
}

impl fmt::Display for StreamEncoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for StreamEncoding {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|encoding| encoding.name() == name)
            .ok_or_else(|| "expected h264_hardware, h264_software or raw".into())
    }
}

/// The stream a receiver asks for. The sender captures at this size, rate and
/// menu blending and encodes with exactly this encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamRequest {
    /// Even, 2 to 1920 (H.264: at least 16).
    pub width: u32,
    /// Even, 2 to 1080 (H.264: at least 16).
    pub height: u32,
    /// 1 to 60.
    pub fps: u32,
    /// Capture on-screen menus over the video and, when nothing is playing
    /// (such as the Home screen), the TV's own screens; false: video only.
    pub include_ui: bool,
    pub encoding: StreamEncoding,
}

impl Default for StreamRequest {
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

impl StreamRequest {
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
        even("width", self.width, 1920)?;
        even("height", self.height, 1080)?;
        if !(1..=60).contains(&self.fps) {
            return Err(ValidationError::new("fps", "must be 1 to 60"));
        }
        if self.encoding != StreamEncoding::Raw && (self.width < 16 || self.height < 16) {
            return Err(ValidationError::new(
                "encoding",
                "H.264 needs at least 16x16; use raw for smaller pictures",
            ));
        }
        Ok(())
    }

    /// The encoder this request names; `None` for raw.
    #[must_use]
    pub fn encoder(&self) -> Option<EncoderSettings> {
        self.encoding
            .backend()
            .map(|backend| EncoderSettings::new(backend, self.fps))
    }
}

impl fmt::Display for StreamRequest {
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
        let request = StreamRequest::default();
        request.validate().unwrap();
        assert_eq!(
            request.to_string(),
            "320x180 at 30 fps with menus as h264_hardware"
        );
        for (bad, field) in [
            (
                StreamRequest {
                    width: 321,
                    ..request
                },
                "width",
            ),
            (
                StreamRequest {
                    width: 1922,
                    ..request
                },
                "width",
            ),
            (
                StreamRequest {
                    height: 0,
                    ..request
                },
                "height",
            ),
            (StreamRequest { fps: 61, ..request }, "fps"),
            (
                StreamRequest {
                    width: 8,
                    ..request
                },
                "encoding",
            ),
        ] {
            assert_eq!(bad.validate().unwrap_err().field, field, "{bad:?}");
        }
        StreamRequest {
            width: 8,
            height: 2,
            encoding: StreamEncoding::Raw,
            ..request
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn encodings_name_their_encoder() {
        for encoding in StreamEncoding::ALL {
            assert_eq!(encoding.name().parse(), Ok(encoding));
            assert_eq!(StreamEncoding::from_code(encoding.code()), Some(encoding));
            assert_eq!(
                serde_json::to_value(encoding).unwrap(),
                serde_json::Value::String(encoding.name().into())
            );
        }
        assert_eq!(StreamEncoding::from_code(3), None);
        assert!("lz4".parse::<StreamEncoding>().is_err());
        let request = |encoding| StreamRequest {
            encoding,
            ..StreamRequest::default()
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
