//! What a sender can deliver: the modes of each encoding it has. It travels
//! in the upgrade request's [`CAPTURE_OFFER_HEADER`](prysm_ports::CAPTURE_OFFER_HEADER),
//! so the portal learns it even while it refuses the stream.
use crate::request::{StreamEncoding, StreamRequest};
use prysm_ports::{MAX_ANNOUNCEMENT_BYTES, Mode, Modes, ValidationError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The sizes and rates a sender streams in each encoding it has, as JSON
/// `{ "h264_hardware": [Mode, ...], "raw": [...] }`; an encoding it lacks is
/// absent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Offer(BTreeMap<StreamEncoding, Vec<Mode>>);

impl Offer {
    /// The offer of these encodings' modes, sorted and without repeats; a
    /// later entry for an encoding replaces an earlier one.
    ///
    /// # Panics
    /// If it does not fit [`MAX_ANNOUNCEMENT_BYTES`], which is a bug in the
    /// sender: it offers a few modes, not a list of every size.
    #[must_use]
    pub fn new(encodings: impl IntoIterator<Item = (StreamEncoding, Vec<Mode>)>) -> Self {
        let offer = Self(
            encodings
                .into_iter()
                .map(|(encoding, modes)| {
                    let modes = modes.into_iter().collect::<BTreeSet<_>>();
                    (encoding, modes.into_iter().collect())
                })
                .collect(),
        );
        assert!(
            offer.header().len() <= MAX_ANNOUNCEMENT_BYTES,
            "a capture offer must fit {MAX_ANNOUNCEMENT_BYTES} bytes"
        );
        offer
    }

    /// What the sender streams in `encoding`: known, empty when it lacks it.
    #[must_use]
    pub fn modes(&self, encoding: StreamEncoding) -> Modes {
        Modes::Known(self.0.get(&encoding).cloned().unwrap_or_default())
    }

    /// Whether the sender streams `request`.
    ///
    /// # Errors
    /// `encoding` when the sender lacks it, else the first of `width`,
    /// `height` and `fps` it does not stream, relative to the section.
    pub fn check(&self, request: &StreamRequest) -> Result<(), ValidationError> {
        let encoding = request.encoding;
        if !self.0.contains_key(&encoding) {
            return Err(ValidationError::new(
                "encoding",
                format!("the sender cannot stream {encoding}"),
            ));
        }
        self.modes(encoding)
            .check(request.width, request.height, request.fps)
    }

    /// As the upgrade request's header value: compact ASCII JSON.
    pub(crate) fn header(&self) -> String {
        serde_json::to_string(self).expect("offers serialize")
    }

    /// The offer in an announcement; `None` for anything else.
    pub(crate) fn parse(announcement: &str) -> Option<Self> {
        serde_json::from_str(announcement).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_ports::Span;
    use serde_json::json;

    fn span(min: u32, max: u32, step: u32) -> Span {
        Span::new(min, max, step).unwrap()
    }

    #[test]
    fn offers_are_encoding_maps_and_check_requests() {
        let hardware = Mode::new(span(176, 1920, 2), span(144, 1080, 2), span(1, 30, 1));
        let offer = Offer::new([
            (StreamEncoding::H264Hardware, vec![hardware, hardware]),
            (StreamEncoding::Raw, Vec::new()),
        ]);
        let header = offer.header();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&header).unwrap(),
            json!({
                "h264_hardware": [{
                    "width": { "min": 176, "max": 1920, "step": 2 },
                    "height": { "min": 144, "max": 1080, "step": 2 },
                    "fps": { "min": 1, "max": 30, "step": 1 },
                }],
                "raw": [],
            })
        );
        assert!(header.is_ascii() && !header.contains(' '));
        assert_eq!(Offer::parse(&header), Some(offer.clone()));
        for bad in ["", "[]", r#"{"lz4":[]}"#, r#"{"raw":[{"width":1}]}"#] {
            assert_eq!(Offer::parse(bad), None, "{bad}");
        }

        let request = StreamRequest::default();
        assert_eq!(offer.check(&request), Ok(()));
        let field = |request: StreamRequest| offer.check(&request).unwrap_err().field;
        assert_eq!(field(StreamRequest { fps: 60, ..request }), "fps");
        assert_eq!(
            field(StreamRequest {
                width: 160,
                ..request
            }),
            "width"
        );
        let software = StreamRequest {
            encoding: StreamEncoding::H264Software,
            ..request
        };
        assert_eq!(
            offer.check(&software),
            Err(ValidationError::new(
                "encoding",
                "the sender cannot stream h264_software"
            ))
        );
        assert_eq!(
            offer.modes(StreamEncoding::H264Software),
            Modes::Known(Vec::new())
        );
        // Offered without modes: nothing streams in it.
        let raw = StreamRequest {
            encoding: StreamEncoding::Raw,
            ..request
        };
        assert_eq!(field(raw), "width");
    }
}
