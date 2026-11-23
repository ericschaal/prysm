//! The capture stream's encodings: what the `remote` source asks its sender
//! for, and what the slim TV worker reports it sends.
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// How the sender encodes the frames it sends: `h264_hardware`, the
/// sender's hardware H.264 encoder (the webOS TV's), about 2 Mbit/s;
/// `h264_software`, OpenH264 on the sender's CPU, about 2 Mbit/s; `raw`,
/// uncompressed NV12 pixels, about 21 Mbit/s at 320x180 and 30 fps. Exactly
/// this encoder is used: one that cannot start or fails stops the stream,
/// and the sender never switches to another.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
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
    pub const ALL: [Self; 3] = [Self::H264Hardware, Self::H264Software, Self::Raw];

    /// The serialized name, such as `h264_hardware`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::H264Hardware => "h264_hardware",
            Self::H264Software => "h264_software",
            Self::Raw => "raw",
        }
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
            .ok_or_else(|| format!("expected one of {}", Self::ALL.map(Self::name).join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use super::StreamEncoding;

    #[test]
    fn names_are_the_serialized_names_and_parse_back() {
        for encoding in StreamEncoding::ALL {
            assert_eq!(
                serde_json::to_value(encoding).unwrap(),
                serde_json::Value::from(encoding.name())
            );
            assert_eq!(encoding.name().parse::<StreamEncoding>(), Ok(encoding));
        }
        assert!("h265".parse::<StreamEncoding>().is_err());
    }
}
