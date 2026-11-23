//! Events of `GET /api/v1/preview/stream`: the captured picture and its
//! sampled area, for a browser.
use serde::{Deserialize, Serialize};

/// One server-sent event of the preview stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum PreviewEvent {
    /// A new processed frame.
    Frame(PreviewFrame),
    /// Nothing to show: no session runs or it has not processed a frame yet.
    Idle,
    /// The latest frame cannot be shown, such as an empty frame.
    Unavailable { message: String },
}

/// A downscaled JPEG of the captured frame and the sampled area.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct PreviewFrame {
    /// Increases with every new frame, across sessions; equal means unchanged.
    pub sequence: u64,
    /// The captured frame's size in pixels; `viewport` is in these pixels.
    pub source_width: u32,
    pub source_height: u32,
    /// The JPEG's size in pixels: the source scaled to at most 480 on its
    /// longer side, keeping its aspect ratio.
    pub width: u32,
    pub height: u32,
    /// The part of the frame that was sampled, after black-bar removal.
    pub viewport: PreviewViewport,
    /// Base64 (standard alphabet, padded) baseline JPEG.
    pub jpeg: String,
}

/// A rectangle in source-frame pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct PreviewViewport {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn events_are_tagged_by_type() {
        assert_eq!(
            serde_json::to_value(PreviewEvent::Idle).unwrap(),
            json!({ "type": "idle" })
        );
        let frame = PreviewEvent::Frame(PreviewFrame {
            sequence: 9,
            source_width: 4,
            source_height: 2,
            width: 4,
            height: 2,
            viewport: PreviewViewport {
                x: 0,
                y: 1,
                width: 4,
                height: 1,
            },
            jpeg: "/9j/".into(),
        });
        let value = serde_json::to_value(&frame).unwrap();
        assert_eq!(value["type"], "frame");
        assert_eq!(value["viewport"]["y"], 1);
        assert_eq!(
            serde_json::from_value::<PreviewEvent>(value).unwrap(),
            frame
        );
    }
}
