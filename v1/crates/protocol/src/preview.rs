//! Events of `GET /api/v1/preview/stream`: what the LEDs show, for a browser.
use serde::{Deserialize, Serialize};

/// One server-sent event of the preview stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum PreviewEvent {
    /// A new processed frame and the colors written for it.
    Frame(PreviewFrame),
    /// Nothing to show: no session runs or it has not processed a frame yet.
    Idle,
    /// The latest frame cannot be shown, such as an undecoded MJPEG frame.
    Unavailable { message: String },
}

/// A downscaled JPEG of the captured frame, the sampled area and the colors.
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
    pub colors: PreviewColors,
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

/// The colors written to the LEDs, in display (sRGB) space and screen
/// orientation: `top` and `bottom` left to right, `left` and `right` top to
/// bottom. Each edge has at most 256 entries spread evenly along it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct PreviewColors {
    pub top: Vec<PreviewColor>,
    pub right: Vec<PreviewColor>,
    pub bottom: Vec<PreviewColor>,
    pub left: Vec<PreviewColor>,
}

/// `[r, g, b]`, each 0–255 sRGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PreviewColor(pub [u8; 3]);

#[cfg(feature = "schema")]
impl utoipa::PartialSchema for PreviewColor {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::schema::{ArrayBuilder, ObjectBuilder, Type};
        ArrayBuilder::new()
            .prefix_items([(); 3].map(|()| {
                ObjectBuilder::new()
                    .schema_type(Type::Integer)
                    .minimum(Some(0.0))
                    .maximum(Some(255.0))
                    .build()
            }))
            .min_items(Some(3))
            .max_items(Some(3))
            .into()
    }
}

#[cfg(feature = "schema")]
impl utoipa::ToSchema for PreviewColor {}

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
            colors: PreviewColors {
                top: vec![PreviewColor([1, 2, 3])],
                right: vec![],
                bottom: vec![],
                left: vec![],
            },
            jpeg: "/9j/".into(),
        });
        let value = serde_json::to_value(&frame).unwrap();
        assert_eq!(value["type"], "frame");
        assert_eq!(value["colors"]["top"], json!([[1, 2, 3]]));
        assert_eq!(value["viewport"]["y"], 1);
        assert_eq!(
            serde_json::from_value::<PreviewEvent>(value).unwrap(),
            frame
        );
    }
}
