//! What travels over the capture stream's WebSocket (see the crate README).
//! The protocol version is the WebSocket subprotocol, agreed at the
//! upgrade. Text messages carry JSON: the receiver's request and keyframe
//! requests, the sender's refusal. Binary messages are frames: a short
//! header, then the payload, the message's length being the payload's.
//!
//! Messages are read tolerantly: unknown fields and message types are
//! ignored, so a peer of the same version may add them.
use crate::request::{RemoteSettings, StreamEncoding};
use prysm_core::{FrameError, PixelFormat, YuvRange, YuvStandardMatrix};
use prysm_ports::{CAPTURE_PROTOCOL_PREFIX, CAPTURE_VERSION, ValidationError};
use serde::{Deserialize, Serialize};
use std::fmt;

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
/// Widest frame or request.
pub(crate) const MAX_WIDTH: u32 = 1920;
/// Tallest frame or request.
pub(crate) const MAX_HEIGHT: u32 = 1080;
/// Highest requested frame rate; the lowest is 1.
pub(crate) const MAX_FPS: u32 = 60;
/// Largest payload: the largest raw frame (RGB at the largest size) or the
/// largest H.264 access unit, whichever is larger.
pub(crate) const MAX_PAYLOAD_BYTES: usize = {
    let raw = MAX_WIDTH as usize * MAX_HEIGHT as usize * 3;
    if raw > prysm_h264::MAX_ACCESS_UNIT_BYTES {
        raw
    } else {
        prysm_h264::MAX_ACCESS_UNIT_BYTES
    }
};
/// Longest refusal reason, in bytes of UTF-8.
pub(crate) const MAX_REASON_BYTES: usize = 1024;
/// Longest text message: a request, a keyframe request or a refusal.
pub(crate) const MAX_TEXT_BYTES: usize = 4 * 1024;

/// Frame header flag: the first picture of an H.264 encoder session (SPS,
/// PPS and IDR); the receiver decodes it with fresh state. Senders write the
/// other bits as 0; receivers ignore them.
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

/// The receiver's text messages, as the sender reads them: its request (the
/// first message), then keyframe requests:
/// `{"type":"request","width":320,...}`, `{"type":"keyframe"}`. A type this
/// end does not know is [`FromReceiver::Unknown`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum FromReceiver {
    Request(RequestMessage),
    /// The receiver cannot decode: start a new H.264 session (a keyframe).
    Keyframe,
    #[serde(other)]
    Unknown,
}

/// The receiver's text messages, as it sends them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ToSender {
    Request(RequestMessage),
    Keyframe,
}

/// The request as it travels: [`RemoteSettings`]' fields, every one
/// required. The encoding is parsed with the settings, so one this end does
/// not know is an invalid `encoding`, not a message that fails to decode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RequestMessage {
    width: u32,
    height: u32,
    fps: u32,
    include_ui: bool,
    encoding: String,
}

impl From<RemoteSettings> for RequestMessage {
    fn from(settings: RemoteSettings) -> Self {
        let RemoteSettings {
            width,
            height,
            fps,
            include_ui,
            encoding,
        } = settings;
        Self {
            width,
            height,
            fps,
            include_ui,
            encoding: encoding.name().to_owned(),
        }
    }
}

impl TryFrom<RequestMessage> for RemoteSettings {
    type Error = ValidationError;

    /// The settings the message asks for, validated.
    fn try_from(message: RequestMessage) -> Result<Self, ValidationError> {
        let RequestMessage {
            width,
            height,
            fps,
            include_ui,
            encoding,
        } = message;
        let settings = Self {
            width,
            height,
            fps,
            include_ui,
            encoding: encoding
                .parse()
                .map_err(|reason| ValidationError::new("encoding", reason))?,
        };
        settings.validate()?;
        Ok(settings)
    }
}

/// What a sender's refusal names as the cause; a cause this end does not
/// know is read as [`RefusalCause::Request`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RefusalCause {
    /// The requested encoding: its encoder cannot start or failed.
    Encoding,
    /// The request as a whole.
    #[serde(other)]
    Request,
}

/// The sender's text messages, as the receiver reads them:
/// `{"type":"refused","cause":"encoding","reason":"..."}`, after which the
/// sender closes the connection. A type this end does not know is
/// [`FromSender::Unknown`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum FromSender {
    Refused(Refusal),
    #[serde(other)]
    Unknown,
}

/// The sender's text messages, as it sends them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ToReceiver {
    Refused(Refusal),
}

/// The sender cannot produce the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Refusal {
    pub(crate) cause: RefusalCause,
    /// In the sender's words; at most [`MAX_REASON_BYTES`].
    pub(crate) reason: String,
}

impl Refusal {
    /// A refusal for `reason`, cut at a character boundary to
    /// [`MAX_REASON_BYTES`].
    pub(crate) fn new(cause: RefusalCause, reason: &str) -> Self {
        Self {
            cause,
            reason: reason[..reason.floor_char_boundary(MAX_REASON_BYTES)].to_owned(),
        }
    }
}

/// Why the receiver ended a stream, as the WebSocket close code it sent.
/// Any other close, or none, is a dropped connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Close {
    /// The receiving source closed (lighting stopped, or its capture
    /// settings changed): code 4000. The sender releases its capture and
    /// encoder until a receiver takes a connection again.
    Stopped,
    /// Another sender's connection replaced this one: code 4001. The sender
    /// stops until its user starts it again, so two senders never take the
    /// stream from each other in turn.
    Replaced,
}

impl Close {
    pub(crate) const fn code(self) -> u16 {
        match self {
            Self::Stopped => 4000,
            Self::Replaced => 4001,
        }
    }

    pub(crate) fn from_code(code: u16) -> Option<Self> {
        [Self::Stopped, Self::Replaced]
            .into_iter()
            .find(|close| close.code() == code)
    }
}

impl fmt::Display for Close {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Stopped => "Prysm stopped receiving",
            Self::Replaced => "another sender replaced this one at Prysm",
        })
    }
}

/// Why a stream or frame cannot be encoded, or was rejected.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum WireError {
    /// The two ends speak different protocol versions; the older one needs
    /// updating. `theirs` is the highest version the peer named, `None`
    /// when it named none.
    #[error(
        "the capture stream versions differ (Prysm: {}, this end: {ours}); update the older one",
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
    /// The peer closed the stream, saying why when it is a [`Close`].
    #[error("{}", .0.map_or_else(|| "the peer closed the stream".to_owned(), |close| close.to_string()))]
    Closed(Option<Close>),
    #[error("a text message of {0} bytes exceeds {MAX_TEXT_BYTES}")]
    TextTooLarge(usize),
    #[error("a frame of {0} bytes is shorter than its {HEADER_LEN}-byte header")]
    Short(usize),
    #[error("unknown pixel format code {format}/{range}/{matrix}")]
    FormatCode { format: u8, range: u8, matrix: u8 },
    #[error("unsupported pixel format `{0}`")]
    Format(PixelFormat),
    #[error("`{0}` in an H.264 stream (limited-range BT.709 NV12 only)")]
    NotH264(PixelFormat),
    #[error("frame {width}x{height} is outside 1x1 to {MAX_WIDTH}x{MAX_HEIGHT}")]
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

/// The version mismatch with a peer that names the subprotocols `theirs`
/// (a comma-separated list, as a `426` gives it).
pub(crate) fn version_error(theirs: Option<&str>) -> WireError {
    WireError::Version {
        ours: CAPTURE_VERSION,
        theirs: theirs.and_then(|list| {
            list.split(',')
                .filter_map(|protocol| {
                    protocol
                        .trim()
                        .strip_prefix(CAPTURE_PROTOCOL_PREFIX)?
                        .parse()
                        .ok()
                })
                .max()
        }),
    }
}

/// A receiver's message, as its text message.
pub(crate) fn encode_to_sender(message: &ToSender) -> String {
    serde_json::to_string(message).expect("receiver messages serialize")
}

/// A receiver's message from its text message.
pub(crate) fn decode_from_receiver(text: &str) -> Result<FromReceiver, WireError> {
    serde_json::from_str(text).map_err(|error| WireError::Json(error.to_string()))
}

/// A sender's message, as its text message.
pub(crate) fn encode_to_receiver(message: &ToReceiver) -> String {
    serde_json::to_string(message).expect("sender messages serialize")
}

/// A sender's message from its text message; a refusal's reason is bounded.
pub(crate) fn decode_from_sender(text: &str) -> Result<FromSender, WireError> {
    let message: FromSender =
        serde_json::from_str(text).map_err(|error| WireError::Json(error.to_string()))?;
    if let FromSender::Refused(refusal) = &message
        && refusal.reason.len() > MAX_REASON_BYTES
    {
        return Err(WireError::Json(format!(
            "a reason of {} bytes, more than {MAX_REASON_BYTES}",
            refusal.reason.len()
        )));
    }
    Ok(message)
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
        // Within the largest size, checked above; an error rather than a clamp.
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
    /// an access unit of 1 to [`MAX_PAYLOAD_BYTES`]. Both: 1x1 to
    /// [`MAX_WIDTH`]x[`MAX_HEIGHT`], within which every raw frame fits the
    /// payload limit.
    fn check_payload(&self, encoding: Encoding, len: usize) -> Result<(), WireError> {
        let Self { width, height, .. } = *self;
        if !(1..=MAX_WIDTH).contains(&width) || !(1..=MAX_HEIGHT).contains(&height) {
            return Err(WireError::Dimensions { width, height });
        }
        let raw = self
            .format
            .expected_size(width, height)
            .map_err(WireError::Geometry)?
            .ok_or(WireError::Format(self.format))?;
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
/// before the caller allocates for it. Flags other than an H.264 stream's
/// keyframe are ignored.
pub(crate) fn decode_frame(
    message: &[u8],
    encoding: Encoding,
) -> Result<FrameMessage<'_>, WireError> {
    let (head, payload) = message
        .split_first_chunk::<HEADER_LEN>()
        .ok_or(WireError::Short(message.len()))?;
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
        keyframe: encoding == Encoding::H264 && head[at::FLAGS] & KEYFRAME != 0,
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
        // The largest raw frame fits.
        let largest = header(MAX_WIDTH, MAX_HEIGHT, PixelFormat::RGB24);
        let len = PixelFormat::RGB24
            .expected_size(MAX_WIDTH, MAX_HEIGHT)
            .unwrap()
            .unwrap();
        assert!(len <= MAX_PAYLOAD_BYTES);
        largest.encode(Encoding::Raw, len).unwrap();
    }

    #[test]
    fn unsendable_frames_are_rejected() {
        for (format, width, height) in [
            (PixelFormat::MJPEG, 2, 2),
            (
                PixelFormat::YUYV {
                    range: YuvRange::Full,
                    matrix: YuvStandardMatrix::Custom(0.2, 0.1),
                },
                2,
                2,
            ),
            (NV12, 3, 2),
            (PixelFormat::RGB24, 0, 2),
            (PixelFormat::RGB24, MAX_WIDTH + 1, 2),
            (PixelFormat::RGB24, 2, MAX_HEIGHT + 1),
        ] {
            let header = header(width, height, format);
            let len = format
                .expected_size(width, height)
                .ok()
                .flatten()
                .unwrap_or(0);
            assert!(
                header.encode(Encoding::Raw, len).is_err(),
                "{format:?} {width}x{height}"
            );
        }
        // A raw payload of another size, H.264 of other pixels, empty or
        // over the payload limit.
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
        assert_eq!(
            header(16, 16, NV12).encode(Encoding::H264, MAX_PAYLOAD_BYTES + 1),
            Err(WireError::TooLarge(MAX_PAYLOAD_BYTES + 1))
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
        assert!(matches!(
            with(4, &[0, 0]),
            Err(WireError::Dimensions { width: 0, .. })
        ));
        // One past the largest size, from the header alone.
        assert!(matches!(
            with(4, &[0x07, 0x81]),
            Err(WireError::Dimensions { width: 1921, .. })
        ));
        assert!(matches!(
            with(6, &[0x04, 0x39]),
            Err(WireError::Dimensions { height: 1081, .. })
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
    fn unknown_flag_bits_are_ignored() {
        let mut raw = message(&header(4, 2, PixelFormat::RGB24), Encoding::Raw, &[0; 24]);
        raw[at::FLAGS] = 0xff;
        assert_eq!(
            decode_frame(&raw, Encoding::Raw).unwrap().header,
            header(4, 2, PixelFormat::RGB24),
            "no keyframe in a raw stream"
        );
        let mut h264 = message(&header(16, 16, NV12), Encoding::H264, &[0; 7]);
        h264[at::FLAGS] = 0xfe;
        assert!(!decode_frame(&h264, Encoding::H264).unwrap().header.keyframe);
        h264[at::FLAGS] = 0xff;
        assert!(decode_frame(&h264, Encoding::H264).unwrap().header.keyframe);
    }

    #[test]
    fn h264_frames_carry_the_keyframe_flag_and_any_length() {
        let key = Header {
            keyframe: true,
            ..header(640, 280, NV12)
        };
        let bytes = message(&key, Encoding::H264, &[0; 1234]);
        assert_eq!(bytes[at::FLAGS], KEYFRAME);
        let got = decode_frame(&bytes, Encoding::H264).unwrap();
        assert_eq!((got.header, got.payload.len()), (key, 1234));
        let plain = header(640, 280, NV12);
        let bytes = message(&plain, Encoding::H264, &[0; 7]);
        assert_eq!(decode_frame(&bytes, Encoding::H264).unwrap().header, plain);
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

    const SOFTWARE: RemoteSettings = RemoteSettings {
        width: 640,
        height: 280,
        fps: 25,
        include_ui: false,
        encoding: StreamEncoding::H264Software,
    };

    /// The request a command carries, as settings.
    fn settings(text: &str) -> Result<RemoteSettings, ValidationError> {
        match decode_from_receiver(text) {
            Ok(FromReceiver::Request(request)) => RemoteSettings::try_from(request),
            other => panic!("{text}: {other:?}"),
        }
    }

    #[test]
    fn commands_are_tagged_json_and_requests_are_complete() {
        let text = encode_to_sender(&ToSender::Request(SOFTWARE.into()));
        assert_eq!(
            text,
            r#"{"type":"request","width":640,"height":280,"fps":25,"include_ui":false,"encoding":"h264_software"}"#
        );
        assert_eq!(settings(&text), Ok(SOFTWARE));
        assert_eq!(
            encode_to_sender(&ToSender::Keyframe),
            r#"{"type":"keyframe"}"#
        );
        assert_eq!(
            decode_from_receiver(r#"{"type":"keyframe"}"#),
            Ok(FromReceiver::Keyframe)
        );
        // Every field is required; a request that is not JSON or not a
        // request fails to decode.
        for bad in [
            r#"{"type":"request"}"#,
            r#"{"type":"request","width":640,"height":280,"fps":25,"include_ui":false}"#,
            r#"{"type":"request","width":"640","height":280,"fps":25,"include_ui":false,"encoding":"raw"}"#,
            r#"{"width":640}"#,
            "not json",
        ] {
            assert!(
                matches!(decode_from_receiver(bad), Err(WireError::Json(_))),
                "{bad}"
            );
        }
        // Validated as the settings are.
        assert_eq!(
            settings(r#"{"type":"request","width":321,"height":280,"fps":25,"include_ui":false,"encoding":"raw"}"#)
                .unwrap_err()
                .field,
            "width"
        );
    }

    /// Unknown fields and message types are ignored; an unknown encoding is
    /// an invalid `encoding`, not a message that fails to decode.
    #[test]
    fn commands_tolerate_what_this_end_does_not_know() {
        assert_eq!(
            settings(
                r#"{"type":"request","width":640,"height":280,"fps":25,"include_ui":false,"encoding":"h264_software","hdr":true}"#
            ),
            Ok(SOFTWARE)
        );
        assert_eq!(
            decode_from_receiver(r#"{"type":"keyframe","reason":"lost"}"#),
            Ok(FromReceiver::Keyframe)
        );
        for unknown in [
            r#"{"type":"jump"}"#,
            r#"{"type":"pause"}"#,
            r#"{"type":"jump","x":1}"#,
        ] {
            assert_eq!(
                decode_from_receiver(unknown),
                Ok(FromReceiver::Unknown),
                "{unknown}"
            );
        }
        assert_eq!(
            settings(
                r#"{"type":"request","width":640,"height":280,"fps":25,"include_ui":false,"encoding":"av1"}"#
            ),
            Err(ValidationError::new(
                "encoding",
                "expected one of h264_hardware, h264_software, raw"
            ))
        );
    }

    #[test]
    fn refusals_carry_a_bounded_reason_and_a_typed_cause() {
        let refusal = Refusal::new(
            RefusalCause::Encoding,
            "hardware H.264 encoder cannot start",
        );
        let text = encode_to_receiver(&ToReceiver::Refused(refusal.clone()));
        assert_eq!(
            text,
            r#"{"type":"refused","cause":"encoding","reason":"hardware H.264 encoder cannot start"}"#
        );
        assert_eq!(decode_from_sender(&text), Ok(FromSender::Refused(refusal)));
        // Cut at a character boundary.
        let long = "é".repeat(MAX_REASON_BYTES);
        let cut = Refusal::new(RefusalCause::Request, &long);
        assert_eq!(cut.reason.len(), MAX_REASON_BYTES);
        assert!(decode_from_sender(&encode_to_receiver(&ToReceiver::Refused(cut))).is_ok());
        for bad in [
            r#"{"type":"refused","reason":"x"}"#,
            r#"{"type":"refused","cause":"request"}"#,
            r#"{"cause":"request","reason":"x"}"#,
            "not json",
        ] {
            assert!(
                matches!(decode_from_sender(bad), Err(WireError::Json(_))),
                "{bad}"
            );
        }
        let oversized = format!(
            r#"{{"type":"refused","cause":"request","reason":"{}"}}"#,
            "x".repeat(MAX_REASON_BYTES + 1)
        );
        assert!(matches!(
            decode_from_sender(&oversized),
            Err(WireError::Json(_))
        ));
    }

    /// An unknown cause is read as `request`, an unknown type or field is
    /// ignored.
    #[test]
    fn sender_messages_tolerate_what_this_end_does_not_know() {
        assert_eq!(
            decode_from_sender(r#"{"type":"refused","cause":"thermal","reason":"x","code":7}"#),
            Ok(FromSender::Refused(Refusal::new(
                RefusalCause::Request,
                "x"
            )))
        );
        assert_eq!(
            decode_from_sender(r#"{"type":"stats","sent":10}"#),
            Ok(FromSender::Unknown)
        );
        // Known causes keep their names.
        assert_eq!(
            serde_json::to_string(&RefusalCause::Request).unwrap(),
            r#""request""#
        );
    }

    #[test]
    fn close_codes_are_typed() {
        for close in [Close::Stopped, Close::Replaced] {
            assert_eq!(Close::from_code(close.code()), Some(close));
        }
        assert_eq!(Close::Stopped.code(), 4000);
        assert_eq!(Close::Replaced.code(), 4001);
        assert_eq!(Close::from_code(1000), None);
        assert_eq!(
            WireError::Closed(None).to_string(),
            "the peer closed the stream"
        );
        assert_eq!(
            WireError::Closed(Some(Close::Replaced)).to_string(),
            "another sender replaced this one at Prysm"
        );
    }

    #[test]
    fn versions_are_read_from_the_subprotocols() {
        assert_eq!(
            version_error(Some("prysm-capture.v3")),
            WireError::Version {
                ours: CAPTURE_VERSION,
                theirs: Some(3)
            }
        );
        // A list names the highest; unknown entries are skipped.
        assert_eq!(
            version_error(Some(
                "prysm-capture.v7, chat,prysm-capture.v9 ,prysm-capture.v8"
            )),
            WireError::Version {
                ours: CAPTURE_VERSION,
                theirs: Some(9)
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
                "the capture stream versions differ (Prysm: unknown, this end: {CAPTURE_VERSION}); update the older one"
            )
        );
    }
}
