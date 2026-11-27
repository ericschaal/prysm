//! What a sender can deliver: the modes of each encoding it has. It travels
//! in the upgrade request's [`CAPTURE_OFFER_HEADER`](prysm_ports::CAPTURE_OFFER_HEADER),
//! so the portal learns it even while it refuses the stream.
use crate::request::RemoteSettings;
use prysm_ports::StreamEncoding;
use prysm_ports::{MAX_ANNOUNCEMENT_BYTES, Mode, Modes, Span, ValidationError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The sizes and rates a sender streams in each encoding it has (an
/// encoding it lacks is absent), and its Prysm release, as JSON
/// `{ "encodings": { "h264_hardware": [Mode, ...], "raw": [...] },
/// "prysm": "1.0.0" }`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Offer {
    encodings: BTreeMap<StreamEncoding, Vec<Mode>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prysm: Option<String>,
}

/// An offer as it is read: unknown keys and encodings are skipped, and modes
/// are read without the settings' strictness.
#[derive(Deserialize)]
struct ReadOffer {
    encodings: BTreeMap<String, serde_json::Value>,
    prysm: Option<String>,
}

#[derive(Deserialize)]
struct ReadMode {
    width: ReadSpan,
    height: ReadSpan,
    fps: ReadSpan,
}

#[derive(Deserialize)]
struct ReadSpan {
    min: u32,
    max: u32,
    step: u32,
}

impl ReadMode {
    /// The mode; `None` when a span is not one.
    fn mode(self) -> Option<Mode> {
        let span = |ReadSpan { min, max, step }| Span::new(min, max, step);
        Some(Mode::new(
            span(self.width)?,
            span(self.height)?,
            span(self.fps)?,
        ))
    }
}

impl Offer {
    /// The offer of these encodings' modes, sorted and without repeats; a
    /// later entry for an encoding replaces an earlier one. It names this
    /// build's Prysm release.
    ///
    /// # Panics
    /// If it does not fit [`MAX_ANNOUNCEMENT_BYTES`], which is a bug in the
    /// sender: it offers a few modes, not a list of every size.
    #[must_use]
    pub fn new(encodings: impl IntoIterator<Item = (StreamEncoding, Vec<Mode>)>) -> Self {
        let offer = Self {
            encodings: encodings
                .into_iter()
                .map(|(encoding, modes)| {
                    let modes = modes.into_iter().collect::<BTreeSet<_>>();
                    (encoding, modes.into_iter().collect())
                })
                .collect(),
            prysm: Some(prysm_version::VERSION.to_owned()),
        };
        assert!(
            offer.header().len() <= MAX_ANNOUNCEMENT_BYTES,
            "a capture offer must fit {MAX_ANNOUNCEMENT_BYTES} bytes"
        );
        offer
    }

    /// What the sender streams in `encoding`: known, empty when it lacks it.
    #[must_use]
    pub fn modes(&self, encoding: StreamEncoding) -> Modes {
        Modes::Known {
            modes: self.encodings.get(&encoding).cloned().unwrap_or_default(),
        }
    }

    /// The sender's Prysm release, if it named one.
    #[must_use]
    pub fn prysm(&self) -> Option<&str> {
        self.prysm.as_deref()
    }

    /// Whether the sender streams `request`.
    ///
    /// # Errors
    /// `encoding` when the sender lacks it, else the first of `width`,
    /// `height` and `fps` it does not stream, relative to the section.
    pub fn check(&self, request: &RemoteSettings) -> Result<(), ValidationError> {
        let encoding = request.encoding;
        if !self.encodings.contains_key(&encoding) {
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

    /// The offer in an announcement; `None` for anything else. An encoding
    /// this end does not know, or whose modes do not parse, is skipped.
    pub(crate) fn parse(announcement: &str) -> Option<Self> {
        let ReadOffer { encodings, prysm } = serde_json::from_str(announcement).ok()?;
        let encodings = encodings
            .into_iter()
            .filter_map(|(name, modes)| {
                let encoding = name.parse::<StreamEncoding>().ok()?;
                let modes = serde_json::from_value::<Vec<ReadMode>>(modes).ok()?;
                let modes = modes
                    .into_iter()
                    .map(ReadMode::mode)
                    .collect::<Option<_>>()?;
                Some((encoding, modes))
            })
            .collect();
        Some(Self { encodings, prysm })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn span(min: u32, max: u32, step: u32) -> Span {
        Span::new(min, max, step).unwrap()
    }

    fn hardware() -> Mode {
        Mode::new(span(176, 1920, 2), span(144, 1080, 2), span(1, 30, 1))
    }

    fn hardware_json() -> serde_json::Value {
        json!({
            "width": { "min": 176, "max": 1920, "step": 2 },
            "height": { "min": 144, "max": 1080, "step": 2 },
            "fps": { "min": 1, "max": 30, "step": 1 },
        })
    }

    #[test]
    fn offers_are_encoding_maps_and_check_requests() {
        let offer = Offer::new([
            (StreamEncoding::H264Hardware, vec![hardware(), hardware()]),
            (StreamEncoding::Raw, Vec::new()),
        ]);
        let header = offer.header();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&header).unwrap(),
            json!({
                "encodings": { "h264_hardware": [hardware_json()], "raw": [] },
                "prysm": prysm_version::VERSION,
            })
        );
        assert!(header.is_ascii() && !header.contains(' '));
        assert_eq!(offer.prysm(), Some(prysm_version::VERSION));
        assert_eq!(Offer::parse(&header), Some(offer.clone()));
        for bad in ["", "[]", "{}", r#"{"encodings":[]}"#] {
            assert_eq!(Offer::parse(bad), None, "{bad}");
        }

        let request = RemoteSettings::default();
        assert_eq!(offer.check(&request), Ok(()));
        let field = |request: RemoteSettings| offer.check(&request).unwrap_err().field;
        assert_eq!(field(RemoteSettings { fps: 60, ..request }), "fps");
        assert_eq!(
            field(RemoteSettings {
                width: 160,
                ..request
            }),
            "width"
        );
        let software = RemoteSettings {
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
            Modes::Known { modes: Vec::new() }
        );
        let raw = RemoteSettings {
            encoding: StreamEncoding::Raw,
            ..request
        };
        assert_eq!(field(raw), "width");
    }

    /// Unknown keys, encodings and mode fields are skipped; the release is
    /// optional.
    #[test]
    fn offers_tolerate_what_this_end_does_not_know() {
        let mut mode = hardware_json();
        mode["hdr"] = json!(true);
        mode["width"]["unit"] = json!("px");
        let announcement = json!({
            "encodings": {
                "h264_hardware": [mode],
                "av1": [{ "profile": "main" }],
                "h264_software": [{ "width": 1 }],
                "raw": [],
            },
            "battery": 80,
        })
        .to_string();
        let offer = Offer::parse(&announcement).unwrap();
        assert_eq!(offer.prysm(), None);
        let software = RemoteSettings {
            encoding: StreamEncoding::H264Software,
            ..RemoteSettings::default()
        };
        assert_eq!(offer.check(&software).unwrap_err().field, "encoding");
        assert_eq!(
            offer.modes(StreamEncoding::H264Hardware),
            Modes::from(hardware())
        );
        assert_eq!(
            offer.modes(StreamEncoding::Raw),
            Modes::Known { modes: Vec::new() }
        );
        assert!(offer.check(&RemoteSettings::default()).is_ok());
    }
}
