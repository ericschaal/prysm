//! JSON messages sent by the demand-driven preview WebSocket.
use crate::ConfigRevision;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PreviewMessage {
    Frame(PreviewFrame),
    Idle,
    Busy,
    Error { message: String },
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PreviewFrame {
    pub frame_id: u64,
    pub revision: ConfigRevision,
    pub source_width: u32,
    pub source_height: u32,
    pub width: u32,
    pub height: u32,
    #[schema(required = true)]
    pub viewport: Option<PreviewViewport>,
    pub colors: PreviewColors,
    pub jpeg: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PreviewViewport {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PreviewColors {
    pub top: Vec<PreviewColor>,
    pub right: Vec<PreviewColor>,
    pub bottom: Vec<PreviewColor>,
    pub left: Vec<PreviewColor>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PreviewColor(pub [u8; 3]);

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
impl ToSchema for PreviewColor {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn messages_keep_the_existing_wire_format() {
        for (message, expected) in [
            (PreviewMessage::Idle, json!({"type": "idle"})),
            (PreviewMessage::Busy, json!({"type": "busy"})),
            (
                PreviewMessage::Error {
                    message: "failed".into(),
                },
                json!({"type": "error", "message": "failed"}),
            ),
        ] {
            assert_eq!(serde_json::to_value(message).unwrap(), expected);
        }
        let message = PreviewMessage::Frame(PreviewFrame {
            frame_id: 42,
            revision: ConfigRevision::new(7),
            source_width: 2,
            source_height: 2,
            width: 2,
            height: 2,
            viewport: None,
            colors: PreviewColors {
                top: vec![PreviewColor([1, 2, 3])],
                right: vec![],
                bottom: vec![],
                left: vec![],
            },
            jpeg: "image".into(),
        });
        let value = serde_json::to_value(message).unwrap();
        assert_eq!(
            value,
            json!({"type":"frame", "frame_id":42, "revision":"7", "source_width":2, "source_height":2, "width":2, "height":2, "viewport":null, "colors":{"top":[[1,2,3]],"right":[],"bottom":[],"left":[]}, "jpeg":"image"})
        );
        assert!(matches!(
            serde_json::from_value::<PreviewMessage>(value).unwrap(),
            PreviewMessage::Frame(_)
        ));
    }
}
