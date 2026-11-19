//! What travels over the capture stream's WebSocket (see the crate README).
//! The protocol version is the WebSocket subprotocol, agreed at the
//! upgrade. Text messages carry JSON: the receiver's request and controls,
//! the sender's refusal. Binary messages are frames: a short header, then
//! the payload, the message's length being the payload's.
use crate::request::{StreamEncoding, StreamRequest};
use prysm_core::{FrameError, PixelFormat, YuvRange, YuvStandardMatrix};
use prysm_ports::{CAPTURE_VERSION, ValidationError};
use serde::{Deserialize, Serialize};

/// The frame header: format, range and matrix codes, flags, width and height
/// (big-endian `u16`).
pub(crate) const HEADER_LEN: usize = 8;
/// Where each header field is.
mod at {
    use std::ops::Range;
    pub const FORMAT: usize = 0;
    pub const RANGE: usize = 1;
    pub const MATRIX: usize = 2;
    pub const FLAGS: usize = 3;
    pub const WIDTH: Range<usize> = 4..6;
    pub const HEIGHT: Range<usize> = 6..8;
}
/// Largest accepted width or height.
pub(crate) const MAX_DIMENSION: u32 = 4096;
/// Largest accepted payload: one 1080p RGB frame.
pub(crate) const MAX_PAYLOAD_BYTES: usize = 1920 * 1080 * 3;
/// Longest refusal reason, in bytes of UTF-8.
pub(crate) const MAX_REASON_BYTES: usize = 1024;
/// Longest text message: a request, a control or a refusal.
pub(crate) const MAX_TEXT_BYTES: usize = 4 * 1024;

/// Frame header flag: the first picture of an H.264 encoder session (SPS,
/// PPS and IDR); the receiver decodes it with fresh state.
const KEYFRAME: u8 = 1;

/// How a stream's frame payloads are encoded, fixed per connection by the
/// request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Encoding {
    /// Uncompressed pixels, exactly the frame's size.
    Raw,
    /// One H.264 access unit per frame (Annex B, Baseline); the header's
    /// format is the decoded limited-range BT.709 NV12.
    H264,
}

impl From<StreamEncoding> for Encoding {
    fn from(encoding: StreamEncoding) -> Self {
        match encoding {
            StreamEncoding::Raw => Self::Raw,
            StreamEncoding::H264Hardware | StreamEncoding::H264Software => Self::H264,
        }
    }
}

/// The receiver's text messages: its request (the first message), then
/// controls at any time: `{"type":"request","width":320,...}`,
/// `{"type":"pause"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Command {
    Request(StreamRequest),
    /// Frames are not wanted (lighting stopped): stop encoding and sending.
    Pause,
    /// Frames are wanted again; the next H.264 picture starts a new session.
    Resume,
    /// The receiver cannot decode: start a new H.264 session (a keyframe).
    Keyframe,
}

/// What a sender's refusal names as the cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Refused {
    /// The request as a whole.
    Request,
    /// The requested encoding: its encoder cannot start or failed.
    Encoding,
}

/// The sender's text message when it cannot produce the request,
/// `{"refused":"encoding","reason":"..."}`; it closes the connection after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Refusal {
    pub(crate) refused: Refused,
    /// In the sender's words; at most [`MAX_REASON_BYTES`].
    pub(crate) reason: String,
}

impl Refusal {
    /// A refusal for `reason`, cut at a character boundary to
    /// [`MAX_REASON_BYTES`].
    pub(crate) fn new(refused: Refused, reason: &str) -> Self {
        let mut end = reason.len().min(MAX_REASON_BYTES);
        while !reason.is_char_boundary(end) {
            end -= 1;
        }
        Self {
            refused,
            reason: reason[..end].to_owned(),
        }
    }
}

/// Why a stream or frame cannot be encoded, or was rejected.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum WireError {
    /// The two ends speak different protocol versions; update both.
    /// `theirs` is `None` when the peer did not name a version.
    #[error(
        "the capture stream versions differ (Prysm: {}, this end: {ours}); update both",
        theirs.map_or_else(|| "unknown".to_owned(), |theirs| theirs.to_string())
    )]
    Version { ours: u8, theirs: Option<u8> },
    /// Prysm's portal answered the upgrade with this HTTP status: `503`
    /// while no source receives the stream (lighting off, or the capture is
    /// not `remote`), `404` without the path (no portal, or an older Prysm).
    #[error("Prysm answered HTTP {0} instead of opening the capture stream")]
    Unavailable(u16),
    /// The receiver did not send its request after the upgrade.
    #[error("the receiver did not send its request")]
    NoRequest,
    /// A text message that is not one this end takes, in serde's words.
    #[error("invalid message: {0}")]
    Json(String),
    /// The receiver asked for a stream this end does not support.
    #[error("invalid stream request")]
    Request(#[source] ValidationError),
    /// A message out of turn: a frame to the sender, a request after the
    /// first, a text message where a frame belongs.
    #[error("the peer sent a message this end does not take now")]
    Unexpected,
    /// The sender cannot produce the requested stream, in its own words.
    #[error("{0}")]
    Refused(String),
    #[error("the peer closed the stream")]
    Closed,
    #[error("a frame of {0} bytes is shorter than its {HEADER_LEN}-byte header")]
    Short(usize),
    #[error("invalid frame header flags {0:#04x}")]
    Flags(u8),
    #[error("unknown pixel format code {format}/{range}/{matrix}")]
    FormatCode { format: u8, range: u8, matrix: u8 },
    #[error("unsupported pixel format `{0}`")]
    Format(PixelFormat),
    #[error("`{0}` in an H.264 stream (limited-range BT.709 NV12 only)")]
    NotH264(PixelFormat),
    #[error("frame {width}x{height} is outside 1..={MAX_DIMENSION}")]
    Dimensions { width: u32, height: u32 },
    #[error("payload of {0} bytes exceeds {MAX_PAYLOAD_BYTES}")]
    TooLarge(usize),
    #[error("an encoded picture must not be empty")]
    Empty,
    #[error("payload length {actual} does not match the frame's {expected} bytes")]
    Length { expected: usize, actual: usize },
    #[error("invalid frame geometry")]
    Geometry(#[source] FrameError),
}

/// The version mismatch with a peer that speaks the subprotocol `theirs`.
pub(crate) fn version_error(theirs: Option<&str>) -> WireError {
    WireError::Version {
        ours: CAPTURE_VERSION,
        theirs: theirs
            .and_then(|protocol| protocol.strip_prefix("prysm-capture.v"))
            .and_then(|version| version.parse().ok()),
    }
}

/// A command, as its text message.
pub(crate) fn encode_command(command: &Command) -> String {
    serde_json::to_string(command).expect("commands serialize")
}

/// A command from its text message; a request is validated.
pub(crate) fn decode_command(text: &str) -> Result<Command, WireError> {
    let command: Command =
        serde_json::from_str(text).map_err(|error| WireError::Json(error.to_string()))?;
    if let Command::Request(request) = command {
        request.validate().map_err(WireError::Request)?;
    }
    Ok(command)
}

/// A refusal, as its text message.
pub(crate) fn encode_refusal(refusal: &Refusal) -> String {
    serde_json::to_string(refusal).expect("refusals serialize")
}

/// A refusal from its text message.
pub(crate) fn decode_refusal(text: &str) -> Result<Refusal, WireError> {
    let refusal: Refusal =
        serde_json::from_str(text).map_err(|error| WireError::Json(error.to_string()))?;
    if refusal.reason.len() > MAX_REASON_BYTES {
        return Err(WireError::Json(format!(
            "a reason of {} bytes, more than {MAX_REASON_BYTES}",
            refusal.reason.len()
        )));
    }
    Ok(refusal)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Header {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    /// H.264 only: the first picture of an encoder session.
    pub keyframe: bool,
}

/// A frame message, checked against its header.
#[derive(Debug, PartialEq)]
pub(crate) struct FrameMessage<'a> {
    pub header: Header,
    pub payload: &'a [u8],
}

impl Header {
    /// The header bytes of a frame whose payload is `payload_len` bytes,
    /// after checking that the payload fits the header for `encoding`.
    pub(crate) fn encode(
        &self,
        encoding: Encoding,
        payload_len: usize,
    ) -> Result<[u8; HEADER_LEN], WireError> {
        self.check_payload(encoding, payload_len)?;
        let flags = match encoding {
            Encoding::H264 if self.keyframe => KEYFRAME,
            Encoding::Raw | Encoding::H264 => 0,
        };
        let (format, range, matrix) = format_code(self.format)?;
        // Within MAX_DIMENSION, checked above; an error rather than a clamp.
        let dimension = |value: u32| {
            u16::try_from(value).map_err(|_| WireError::Dimensions {
                width: self.width,
                height: self.height,
            })
        };
        let mut bytes = [0; HEADER_LEN];
        bytes[at::FORMAT] = format;
        bytes[at::RANGE] = range;
        bytes[at::MATRIX] = matrix;
        bytes[at::FLAGS] = flags;
        bytes[at::WIDTH].copy_from_slice(&dimension(self.width)?.to_be_bytes());
        bytes[at::HEIGHT].copy_from_slice(&dimension(self.height)?.to_be_bytes());
        Ok(bytes)
    }

    /// Raw: exactly the frame's size. H.264: limited-range BT.709 NV12 and
    /// an access unit of 1 to [`MAX_PAYLOAD_BYTES`]. Both: dimensions 1 to
    /// [`MAX_DIMENSION`].
    fn check_payload(&self, encoding: Encoding, len: usize) -> Result<(), WireError> {
        let Self { width, height, .. } = *self;
        if !(1..=MAX_DIMENSION).contains(&width) || !(1..=MAX_DIMENSION).contains(&height) {
            return Err(WireError::Dimensions { width, height });
        }
        let raw = self
            .format
            .expected_size(width, height)
            .map_err(WireError::Geometry)?
            .ok_or(WireError::Format(self.format))?;
        if raw > MAX_PAYLOAD_BYTES {
            return Err(WireError::TooLarge(raw));
        }
        match encoding {
            Encoding::Raw => {
                if len != raw {
                    return Err(WireError::Length {
                        expected: raw,
                        actual: len,
                    });
                }
            }
            Encoding::H264 => {
                if self.format != prysm_h264::FORMAT {
                    return Err(WireError::NotH264(self.format));
                }
                if len == 0 {
                    return Err(WireError::Empty);
                }
                if len > MAX_PAYLOAD_BYTES {
                    return Err(WireError::TooLarge(len));
                }
            }
        }
        Ok(())
    }
}

/// Parse one frame message of a stream in `encoding`, checking everything
/// before the caller allocates for it.
pub(crate) fn decode_frame(
    message: &[u8],
    encoding: Encoding,
) -> Result<FrameMessage<'_>, WireError> {
    let (head, payload) = message
        .split_first_chunk::<HEADER_LEN>()
        .ok_or(WireError::Short(message.len()))?;
    let flags = head[at::FLAGS];
    let allowed = match encoding {
        Encoding::Raw => 0,
        Encoding::H264 => KEYFRAME,
    };
    if flags & !allowed != 0 {
        return Err(WireError::Flags(flags));
    }
    let dimension = |range: std::ops::Range<usize>| {
        u32::from(u16::from_be_bytes([
            head[range.start],
            head[range.start + 1],
        ]))
    };
    let header = Header {
        width: dimension(at::WIDTH),
        height: dimension(at::HEIGHT),
        format: parse_format(head[at::FORMAT], head[at::RANGE], head[at::MATRIX])?,
        keyframe: flags & KEYFRAME != 0,
    };
    header.check_payload(encoding, payload.len())?;
    Ok(FrameMessage { header, payload })
}

fn yuv_code(
    format: PixelFormat,
    range: YuvRange,
    matrix: YuvStandardMatrix,
) -> Result<(u8, u8), WireError> {
    let range = match range {
        YuvRange::Limited => 1,
        YuvRange::Full => 2,
    };
    let matrix = match matrix {
        YuvStandardMatrix::Bt601 => 1,
        YuvStandardMatrix::Bt709 => 2,
        YuvStandardMatrix::Bt2020 => 3,
        YuvStandardMatrix::Smpte240 => 4,
        YuvStandardMatrix::Bt470_6 => 5,
        YuvStandardMatrix::Fcc => 6,
        YuvStandardMatrix::Custom(..) => return Err(WireError::Format(format)),
    };
    Ok((range, matrix))
}

fn format_code(format: PixelFormat) -> Result<(u8, u8, u8), WireError> {
    Ok(match format {
        PixelFormat::RGB24 => (1, 0, 0),
        PixelFormat::BGR24 => (2, 0, 0),
        PixelFormat::YUYV { range, matrix } => {
            let (range, matrix) = yuv_code(format, range, matrix)?;
            (3, range, matrix)
        }
        PixelFormat::NV12 { range, matrix } => {
            let (range, matrix) = yuv_code(format, range, matrix)?;
            (4, range, matrix)
        }
        PixelFormat::MJPEG => return Err(WireError::Format(format)),
    })
}

fn parse_format(format: u8, range: u8, matrix: u8) -> Result<PixelFormat, WireError> {
    let unknown = WireError::FormatCode {
        format,
        range,
        matrix,
    };
    let yuv = || -> Result<(YuvRange, YuvStandardMatrix), WireError> {
        Ok((
            match range {
                1 => YuvRange::Limited,
                2 => YuvRange::Full,
                _ => return Err(unknown.clone()),
            },
            match matrix {
                1 => YuvStandardMatrix::Bt601,
                2 => YuvStandardMatrix::Bt709,
                3 => YuvStandardMatrix::Bt2020,
                4 => YuvStandardMatrix::Smpte240,
                5 => YuvStandardMatrix::Bt470_6,
                6 => YuvStandardMatrix::Fcc,
                _ => return Err(unknown.clone()),
            },
        ))
    };
    Ok(match (format, range, matrix) {
        (1, 0, 0) => PixelFormat::RGB24,
        (2, 0, 0) => PixelFormat::BGR24,
        (3, ..) => {
            let (range, matrix) = yuv()?;
            PixelFormat::YUYV { range, matrix }
        }
        (4, ..) => {
            let (range, matrix) = yuv()?;
            PixelFormat::NV12 { range, matrix }
        }
        _ => return Err(unknown),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(width: u32, height: u32, format: PixelFormat) -> Header {
        Header {
            width,
            height,
            format,
            keyframe: false,
        }
    }

    const NV12: PixelFormat = PixelFormat::NV12 {
        range: YuvRange::Limited,
        matrix: YuvStandardMatrix::Bt709,
    };

    /// A frame message: the header, then `payload`.
    fn message(header: &Header, encoding: Encoding, payload: &[u8]) -> Vec<u8> {
        let mut bytes = header.encode(encoding, payload.len()).unwrap().to_vec();
        bytes.extend_from_slice(payload);
        bytes
    }

    #[test]
    fn every_fixed_size_format_round_trips() {
        let yuyv = |range, matrix| PixelFormat::YUYV { range, matrix };
        for format in [
            PixelFormat::RGB24,
            PixelFormat::BGR24,
            yuyv(YuvRange::Limited, YuvStandardMatrix::Bt709),
            yuyv(YuvRange::Full, YuvStandardMatrix::Bt601),
            yuyv(YuvRange::Full, YuvStandardMatrix::Fcc),
            NV12,
            PixelFormat::NV12 {
                range: YuvRange::Full,
                matrix: YuvStandardMatrix::Bt601,
            },
        ] {
            let sent = header(320, 180, format);
            let payload = vec![7; format.expected_size(320, 180).unwrap().unwrap()];
            let bytes = message(&sent, Encoding::Raw, &payload);
            let got = decode_frame(&bytes, Encoding::Raw).unwrap();
            assert_eq!(got.header, sent);
            assert_eq!(got.payload, payload);
        }
        // Eight bytes of header, then the payload.
        let bytes = message(
            &header(2, 1, PixelFormat::RGB24),
            Encoding::Raw,
            &[1, 2, 3, 4, 5, 6],
        );
        assert_eq!(bytes, [1, 0, 0, 0, 0, 2, 0, 1, 1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn unsendable_frames_are_rejected() {
        for (format, width) in [
            (PixelFormat::MJPEG, 2),
            (
                PixelFormat::YUYV {
                    range: YuvRange::Full,
                    matrix: YuvStandardMatrix::Custom(0.2, 0.1),
                },
                2,
            ),
            (NV12, 3),
            (PixelFormat::RGB24, 0),
            (PixelFormat::RGB24, MAX_DIMENSION + 1),
        ] {
            let header = header(width, 2, format);
            let len = format.expected_size(width, 2).ok().flatten().unwrap_or(0);
            assert!(
                header.encode(Encoding::Raw, len).is_err(),
                "{format:?} {width}"
            );
        }
        // Within each dimension limit but over the payload limit.
        assert!(matches!(
            header(4096, 4096, PixelFormat::RGB24).encode(Encoding::Raw, 4096 * 4096 * 3),
            Err(WireError::TooLarge(_))
        ));
        // A raw payload of another size, H.264 of other pixels or empty.
        assert_eq!(
            header(2, 1, PixelFormat::RGB24).encode(Encoding::Raw, 5),
            Err(WireError::Length {
                expected: 6,
                actual: 5
            })
        );
        assert_eq!(
            header(16, 16, PixelFormat::RGB24).encode(Encoding::H264, 10),
            Err(WireError::NotH264(PixelFormat::RGB24))
        );
        assert_eq!(
            header(16, 16, NV12).encode(Encoding::H264, 0),
            Err(WireError::Empty)
        );
    }

    #[test]
    fn malformed_frames_are_rejected_before_allocation() {
        let good = message(&header(4, 2, PixelFormat::RGB24), Encoding::Raw, &[0; 24]);
        let with = |at: usize, patch: &[u8]| {
            let mut bytes = good.clone();
            bytes[at..at + patch.len()].copy_from_slice(patch);
            decode_frame(&bytes, Encoding::Raw).map(|frame| frame.header)
        };
        assert!(matches!(with(0, &[9]), Err(WireError::FormatCode { .. })));
        assert!(matches!(with(1, &[1]), Err(WireError::FormatCode { .. })));
        assert!(matches!(with(3, &[1]), Err(WireError::Flags(1))));
        assert!(matches!(
            with(4, &[0, 0]),
            Err(WireError::Dimensions { width: 0, .. })
        ));
        assert!(matches!(
            with(4, &[0x10, 0x01]),
            Err(WireError::Dimensions { .. })
        ));
        // 4096x4096 RGB is over the payload limit, from the header alone.
        assert!(matches!(
            with(4, &[0x10, 0x00, 0x10, 0x00]),
            Err(WireError::TooLarge(_))
        ));
        // The payload is the rest of the message: one byte short or long.
        assert_eq!(
            decode_frame(&good[..good.len() - 1], Encoding::Raw),
            Err(WireError::Length {
                expected: 24,
                actual: 23
            })
        );
        let mut long = good.clone();
        long.push(0);
        assert!(matches!(
            decode_frame(&long, Encoding::Raw),
            Err(WireError::Length { .. })
        ));
        assert_eq!(
            decode_frame(&good[..7], Encoding::Raw),
            Err(WireError::Short(7))
        );
        // Odd NV12 geometry is not a frame.
        let mut odd = message(&header(4, 2, NV12), Encoding::Raw, &[0; 12]);
        odd[5] = 3;
        assert!(matches!(
            decode_frame(&odd, Encoding::Raw),
            Err(WireError::Geometry(_))
        ));
    }

    #[test]
    fn h264_frames_carry_the_keyframe_flag_and_any_length() {
        let key = Header {
            keyframe: true,
            ..header(640, 280, NV12)
        };
        let bytes = message(&key, Encoding::H264, &[0; 1234]);
        let got = decode_frame(&bytes, Encoding::H264).unwrap();
        assert_eq!((got.header, got.payload.len()), (key, 1234));
        let plain = header(640, 280, NV12);
        let bytes = message(&plain, Encoding::H264, &[0; 7]);
        assert_eq!(decode_frame(&bytes, Encoding::H264).unwrap().header, plain);
        // The keyframe flag is not a raw flag.
        let mut flagged = message(&key, Encoding::H264, &[0; 7]);
        assert!(matches!(
            decode_frame(&flagged, Encoding::Raw),
            Err(WireError::Flags(1))
        ));
        flagged[3] = 2;
        assert!(matches!(
            decode_frame(&flagged, Encoding::H264),
            Err(WireError::Flags(2))
        ));
        assert_eq!(
            decode_frame(&flagged[..HEADER_LEN], Encoding::H264),
            Err(WireError::Flags(2))
        );
        let empty = message(&key, Encoding::H264, &[0; 7]);
        assert_eq!(
            decode_frame(&empty[..HEADER_LEN], Encoding::H264),
            Err(WireError::Empty)
        );
        // Other pixels in an H.264 stream.
        let rgb = message(&header(4, 2, PixelFormat::RGB24), Encoding::Raw, &[0; 24]);
        assert_eq!(
            decode_frame(&rgb, Encoding::H264),
            Err(WireError::NotH264(PixelFormat::RGB24))
        );
    }

    #[test]
    fn commands_are_tagged_json_and_requests_are_validated() {
        let request = StreamRequest {
            width: 640,
            height: 280,
            fps: 25,
            include_ui: false,
            encoding: StreamEncoding::H264Software,
        };
        let text = encode_command(&Command::Request(request));
        assert_eq!(
            text,
            r#"{"type":"request","width":640,"height":280,"fps":25,"include_ui":false,"encoding":"h264_software"}"#
        );
        assert_eq!(decode_command(&text), Ok(Command::Request(request)));
        for (command, text) in [
            (Command::Pause, r#"{"type":"pause"}"#),
            (Command::Resume, r#"{"type":"resume"}"#),
            (Command::Keyframe, r#"{"type":"keyframe"}"#),
        ] {
            assert_eq!(encode_command(&command), text);
            assert_eq!(decode_command(text), Ok(command));
        }
        // Defaults fill a partial request; unknown fields and types are refused.
        assert_eq!(
            decode_command(r#"{"type":"request"}"#),
            Ok(Command::Request(StreamRequest::default()))
        );
        for bad in [
            r#"{"type":"request","listen":"0.0.0.0:7380"}"#,
            r#"{"type":"jump"}"#,
            r#"{"type":"request","encoding":"lz4"}"#,
            "not json",
        ] {
            assert!(
                matches!(decode_command(bad), Err(WireError::Json(_))),
                "{bad}"
            );
        }
        assert!(matches!(
            decode_command(r#"{"type":"request","width":321}"#),
            Err(WireError::Request(invalid)) if invalid.field == "width"
        ));
    }

    #[test]
    fn refusals_carry_a_bounded_reason_and_a_typed_cause() {
        let refusal = Refusal::new(Refused::Encoding, "hardware H.264 encoder cannot start");
        let text = encode_refusal(&refusal);
        assert_eq!(
            text,
            r#"{"refused":"encoding","reason":"hardware H.264 encoder cannot start"}"#
        );
        assert_eq!(decode_refusal(&text), Ok(refusal));
        // Cut at a character boundary.
        let long = "é".repeat(MAX_REASON_BYTES);
        let cut = Refusal::new(Refused::Request, &long);
        assert_eq!(cut.reason.len(), MAX_REASON_BYTES);
        assert!(decode_refusal(&encode_refusal(&cut)).is_ok());
        for bad in [
            r#"{"refused":"other","reason":"x"}"#,
            r#"{"refused":"request","reason":"x","extra":1}"#,
            r#"{"reason":"x"}"#,
        ] {
            assert!(
                matches!(decode_refusal(bad), Err(WireError::Json(_))),
                "{bad}"
            );
        }
        let oversized = format!(
            r#"{{"refused":"request","reason":"{}"}}"#,
            "x".repeat(MAX_REASON_BYTES + 1)
        );
        assert!(matches!(
            decode_refusal(&oversized),
            Err(WireError::Json(_))
        ));
    }

    #[test]
    fn versions_are_read_from_the_subprotocol() {
        assert_eq!(
            version_error(Some("prysm-capture.v3")),
            WireError::Version {
                ours: CAPTURE_VERSION,
                theirs: Some(3)
            }
        );
        assert_eq!(
            version_error(Some("chat")),
            WireError::Version {
                ours: CAPTURE_VERSION,
                theirs: None
            }
        );
        assert_eq!(
            version_error(None).to_string(),
            format!(
                "the capture stream versions differ (Prysm: unknown, this end: {CAPTURE_VERSION}); update both"
            )
        );
    }
}
