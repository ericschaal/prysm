//! Bounded, binary WebSocket control messages.
use crate::VERSION;
use anyhow::{Result, ensure};

pub const MAX_CONTROL_LEN: usize = 128;
pub const HELLO: ControlMessage = ControlMessage::Hello { version: VERSION };

/// Connection control uses bounded JSON; frame payloads stay binary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlMessage {
    Hello {
        version: u32,
    },
    Idle,
    Busy,
    CaptureSettings {
        refresh_rate_hz: u32,
        capture_resolution: [u32; 2],
    },
}

impl ControlMessage {
    pub fn validate(self) -> Result<Self> {
        match self {
            Self::Idle | Self::Busy => {}
            Self::Hello { version } => {
                ensure!(version == VERSION, "unsupported webOS handshake version");
            }
            Self::CaptureSettings {
                refresh_rate_hz,
                capture_resolution: [width, height],
            } => {
                ensure!(
                    (2..=1920).contains(&width)
                        && width.is_multiple_of(2)
                        && (2..=1080).contains(&height)
                        && height.is_multiple_of(2),
                    "invalid webOS capture resolution"
                );
                ensure!(
                    (1..=60).contains(&refresh_rate_hz),
                    "webOS refresh_rate_hz must be between 1 and 60"
                );
            }
        }
        Ok(self)
    }

    pub fn encode(self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(&self.validate()?)?)
    }

    pub fn decode(payload: &[u8]) -> Result<Self> {
        ensure!(
            (1..=MAX_CONTROL_LEN).contains(&payload.len()),
            "invalid webOS control message length"
        );
        serde_json::from_slice::<Self>(payload)?.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_messages_are_versioned_bounded_and_validate_fields() {
        for message in [
            HELLO,
            ControlMessage::Idle,
            ControlMessage::CaptureSettings {
                refresh_rate_hz: 1,
                capture_resolution: [2, 2],
            },
            ControlMessage::CaptureSettings {
                refresh_rate_hz: 60,
                capture_resolution: [1920, 1080],
            },
        ] {
            let bytes = message.encode().unwrap();
            assert_eq!(ControlMessage::decode(&bytes).unwrap(), message);
        }
        for json in [
            r#"{"type":"capture_settings","refresh_rate_hz":0}"#,
            r#"{"type":"capture_settings","refresh_rate_hz":61}"#,
            r#"{"type":"capture_settings","refresh_rate_hz":"30"}"#,
            r#"{"type":"capture_settings"}"#,
            r#"{"type":"unknown"}"#,
            "{",
        ] {
            assert!(ControlMessage::decode(json.as_bytes()).is_err());
        }
        for value in [
            serde_json::json!({"type":"hello", "version":VERSION.wrapping_sub(1)}),
            serde_json::json!({"type":"hello", "version":VERSION + 1}),
            serde_json::json!({"type":"hello", "version":VERSION, "extra":true}),
        ] {
            assert!(ControlMessage::decode(&serde_json::to_vec(&value).unwrap()).is_err());
        }
        assert!(ControlMessage::decode(&[b' '; MAX_CONTROL_LEN + 1]).is_err());
    }

    #[test]
    fn settings_require_bounded_even_dimensions() {
        for resolution in [
            [0, 360],
            [3, 360],
            [640, 1],
            [1922, 360],
            [640, 1082],
            [u32::MAX, u32::MAX],
        ] {
            let payload = serde_json::json!({"type":"capture_settings", "refresh_rate_hz":30, "capture_resolution":resolution});
            assert!(ControlMessage::decode(&serde_json::to_vec(&payload).unwrap()).is_err());
        }
        assert!(
            ControlMessage::decode(br#"{"type":"capture_settings","refresh_rate_hz":30}"#).is_err()
        );
        for rate in [0, 61] {
            assert!(
                ControlMessage::CaptureSettings {
                    refresh_rate_hz: rate,
                    capture_resolution: [640, 360]
                }
                .encode()
                .is_err()
            );
        }
    }
}
