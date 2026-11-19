//! Events of `GET /api/v1/colors/stream`: the colors the LEDs show, without
//! the picture, for a page that only tints with them.
use serde::{Deserialize, Serialize};

/// Colors per edge in a [`ColorsEvent::Colors`]: each is the average of an
/// equal stretch of its edge.
pub const COLOR_SEGMENTS: usize = 8;

/// One server-sent event of the colors stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum ColorsEvent {
    /// The colors last written to the LEDs, [`COLOR_SEGMENTS`] per edge
    /// (fewer when an edge has fewer samples). Sent only when they changed.
    Colors(LedColors),
    /// The LEDs are not showing Prysm's colors: lights off, the device's own
    /// effects, a source without frames, or no frame processed yet.
    Idle,
}

/// The colors written to the LEDs, in display (sRGB) space and screen
/// orientation: `top` and `bottom` left to right, `left` and `right` top to
/// bottom. Each edge has entries spread evenly along it; how many depends on
/// the stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct LedColors {
    pub top: Vec<LedColor>,
    pub right: Vec<LedColor>,
    pub bottom: Vec<LedColor>,
    pub left: Vec<LedColor>,
}

/// `[r, g, b]`, each 0–255 sRGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LedColor(pub [u8; 3]);

#[cfg(feature = "schema")]
impl utoipa::PartialSchema for LedColor {
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
impl utoipa::ToSchema for LedColor {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn events_are_tagged_by_type() {
        assert_eq!(
            serde_json::to_value(ColorsEvent::Idle).unwrap(),
            json!({ "type": "idle" })
        );
        let colors = ColorsEvent::Colors(LedColors {
            top: vec![LedColor([1, 2, 3])],
            right: vec![],
            bottom: vec![LedColor([4, 5, 6])],
            left: vec![],
        });
        let value = serde_json::to_value(&colors).unwrap();
        assert_eq!(
            value,
            json!({
                "type": "colors",
                "top": [[1, 2, 3]],
                "right": [],
                "bottom": [[4, 5, 6]],
                "left": [],
            })
        );
        assert_eq!(
            serde_json::from_value::<ColorsEvent>(value).unwrap(),
            colors
        );
    }
}
