//! The stream hello, its reply with the request, the frame header and the
//! refusal. See the crate README for the protocol.
use crate::request::{StreamEncoding, StreamRequest};
use prysm_core::{FrameError, PixelFormat, YuvRange, YuvStandardMatrix};

pub(crate) const MAGIC: [u8; 4] = *b"PRSM";
/// The wire protocol version both ends must speak.
pub const VERSION: u8 = 3;
/// The hello (sender to receiver) and the first half of the reply.
pub(crate) const HELLO_LEN: usize = 8;
/// The reply: the hello's layout with a status, then the request.
pub(crate) const REPLY_LEN: usize = 16;
pub(crate) const HEADER_LEN: usize = 16;
/// Largest accepted width or height.
pub const MAX_DIMENSION: u32 = 4096;
/// Largest accepted payload: one 1080p RGB frame.
pub const MAX_PAYLOAD_BYTES: usize = 1920 * 1080 * 3;
/// Longest refusal reason, in bytes of UTF-8.
pub const MAX_REASON_BYTES: usize = 1024;

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

/// Receiver-to-sender messages: one byte each, any time after the reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Control {
    /// Frames are not wanted (lighting stopped): stop encoding and sending.
    Pause,
    /// Frames are wanted again; the next H.264 picture starts a new session.
    Resume,
    /// The receiver cannot decode: start a new H.264 session (a keyframe).
    Keyframe,
}

impl Control {
    pub(crate) const fn code(self) -> u8 {
        match self {
            Self::Pause => 1,
            Self::Resume => 2,
            Self::Keyframe => 3,
        }
    }

    pub(crate) fn from_code(code: u8) -> Result<Self, WireError> {
        match code {
            1 => Ok(Self::Pause),
            2 => Ok(Self::Resume),
            3 => Ok(Self::Keyframe),
            _ => Err(WireError::Control(code)),
        }
    }
}

/// Frame header flag: the first picture of an H.264 encoder session (SPS,
/// PPS and IDR); the receiver decodes it with fresh state.
pub(crate) const KEYFRAME: u8 = 1;

/// Why a stream or frame cannot be encoded, or was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum WireError {
    #[error("not a Prysm frame stream")]
    Magic,
    /// The two ends speak different protocol versions; update both.
    #[error("the peer speaks wire protocol version {theirs}, this end version {ours}")]
    Version { ours: u8, theirs: u8 },
    /// The receiver asked for a stream this sender does not understand.
    #[error("invalid stream request: {0}")]
    Request(String),
    /// The sender cannot produce the requested stream, in its own words.
    #[error("{0}")]
    Refused(String),
    #[error("unknown control message {0}")]
    Control(u8),
    #[error("the receiver closed the stream")]
    Closed,
    #[error("invalid frame header flags {0:#04x}")]
    Flags(u8),
    #[error("an encoded payload must be 1 to {MAX_PAYLOAD_BYTES} bytes, not {0}")]
    EncodedLength(usize),
    #[error("a refusal reason must be 1 to {MAX_REASON_BYTES} bytes, not {0}")]
    ReasonLength(usize),
    #[error("receiver rejected the stream with unknown status {0}")]
    Rejected(u8),
    #[error("the receiver did not answer the hello")]
    NoReply,
    #[error("unsupported pixel format `{0}`")]
    Format(String),
    #[error("unknown pixel format code {format}/{range}/{matrix}")]
    FormatCode { format: u8, range: u8, matrix: u8 },
    #[error("frame {width}x{height} is outside 1..={MAX_DIMENSION}")]
    Dimensions { width: u32, height: u32 },
    #[error("payload of {0} bytes exceeds {MAX_PAYLOAD_BYTES}")]
    TooLarge(usize),
    #[error("payload length {actual} does not match the frame's {expected} bytes")]
    Length { expected: usize, actual: usize },
    #[error("invalid frame geometry")]
    Geometry(#[source] FrameError),
}

/// The first bytes on a connection: `PRSM`, version, three zero bytes.
pub(crate) fn hello() -> [u8; HELLO_LEN] {
    let mut bytes = [0; HELLO_LEN];
    bytes[..4].copy_from_slice(&MAGIC);
    bytes[4] = VERSION;
    bytes
}

/// Magic and version of a hello, or of a reply's first half.
fn check_version(bytes: &[u8; HELLO_LEN]) -> Result<(), WireError> {
    if bytes[..4] != MAGIC {
        return Err(WireError::Magic);
    }
    if bytes[4] != VERSION {
        return Err(WireError::Version {
            ours: VERSION,
            theirs: bytes[4],
        });
    }
    Ok(())
}

/// The receiver's check of a hello.
pub(crate) fn accept(bytes: &[u8; HELLO_LEN]) -> Result<(), WireError> {
    check_version(bytes)
}

/// Reply status codes.
const ACCEPTED: u8 = 0;
const UNSUPPORTED_VERSION: u8 = 1;

/// The receiver's answer: `PRSM`, its version, a status, five zero bytes;
/// then, if accepted, the request: encoding, `include_ui` (0 or 1), width,
/// height (`u16` each), fps, one zero byte. A refused stream's request bytes
/// are zero and the receiver closes the connection.
pub(crate) fn reply(outcome: Result<&StreamRequest, &WireError>) -> [u8; REPLY_LEN] {
    let mut bytes = [0; REPLY_LEN];
    bytes[..HELLO_LEN].copy_from_slice(&hello());
    match outcome {
        Ok(request) => {
            let narrow = |value: u32| u16::try_from(value).unwrap_or(u16::MAX).to_be_bytes();
            bytes[5] = ACCEPTED;
            bytes[8] = request.encoding.code();
            bytes[9] = u8::from(request.include_ui);
            bytes[10..12].copy_from_slice(&narrow(request.width));
            bytes[12..14].copy_from_slice(&narrow(request.height));
            bytes[14] = u8::try_from(request.fps).unwrap_or(u8::MAX);
        }
        Err(_) => bytes[5] = UNSUPPORTED_VERSION,
    }
    bytes
}

/// The sender's check of the reply's first half: same version, accepted.
pub(crate) fn accepted(bytes: &[u8; HELLO_LEN]) -> Result<(), WireError> {
    check_version(bytes)?;
    match bytes[5] {
        ACCEPTED => Ok(()),
        status => Err(WireError::Rejected(status)),
    }
}

/// The request in the reply's second half, validated.
pub(crate) fn request(bytes: &[u8; HELLO_LEN]) -> Result<StreamRequest, WireError> {
    let invalid = |why: String| Err(WireError::Request(why));
    let Some(encoding) = StreamEncoding::from_code(bytes[0]) else {
        return invalid(format!("unknown encoding {}", bytes[0]));
    };
    let include_ui = match bytes[1] {
        0 => false,
        1 => true,
        other => return invalid(format!("include_ui {other}")),
    };
    if bytes[7] != 0 {
        return invalid(format!("reserved byte {}", bytes[7]));
    }
    let request = StreamRequest {
        width: u32::from(u16::from_be_bytes([bytes[2], bytes[3]])),
        height: u32::from(u16::from_be_bytes([bytes[4], bytes[5]])),
        fps: u32::from(bytes[6]),
        include_ui,
        encoding,
    };
    request
        .validate()
        .map_err(|error| WireError::Request(error.to_string()))?;
    Ok(request)
}

/// Refusal flag: the requested encoding is what cannot be produced (its
/// encoder cannot start or failed); without it, the request as a whole.
pub(crate) const REFUSED_ENCODING: u8 = 1;

/// A refusal: a header with format `0`, flags (`REFUSED_ENCODING` or 0),
/// zeros, and the reason's length, followed by the reason (UTF-8, cut to
/// [`MAX_REASON_BYTES`]).
pub(crate) fn refusal(reason: &str, flags: u8) -> Vec<u8> {
    let mut end = reason.len().min(MAX_REASON_BYTES);
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    let reason = if end == 0 { "refused" } else { &reason[..end] };
    let mut bytes = vec![0; HEADER_LEN];
    bytes[3] = flags;
    // At most MAX_REASON_BYTES.
    let len = u32::try_from(reason.len()).unwrap_or(0);
    bytes[12..16].copy_from_slice(&len.to_be_bytes());
    bytes.extend_from_slice(reason.as_bytes());
    bytes
}

/// `Some` for a refusal header (format `0`): the reason's length and flags.
pub(crate) fn refusal_len(bytes: &[u8; HEADER_LEN]) -> Option<Result<(usize, u8), WireError>> {
    if bytes[0] != 0 {
        return None;
    }
    if bytes[3] & !REFUSED_ENCODING != 0 {
        return Some(Err(WireError::Flags(bytes[3])));
    }
    if bytes[1..3]
        .iter()
        .chain(&bytes[4..12])
        .any(|&byte| byte != 0)
    {
        return Some(Err(WireError::FormatCode {
            format: 0,
            range: bytes[1],
            matrix: bytes[2],
        }));
    }
    let len = u32::from_be_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    let len = usize::try_from(len).unwrap_or(usize::MAX);
    Some(if (1..=MAX_REASON_BYTES).contains(&len) {
        Ok((len, bytes[3]))
    } else {
        Err(WireError::ReasonLength(len))
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Header {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    /// H.264 only: the first picture of an encoder session.
    pub keyframe: bool,
}

impl Header {
    /// Checks dimensions and size; returns the exact raw payload length.
    pub(crate) fn payload_len(&self) -> Result<usize, WireError> {
        let Self { width, height, .. } = *self;
        if !(1..=MAX_DIMENSION).contains(&width) || !(1..=MAX_DIMENSION).contains(&height) {
            return Err(WireError::Dimensions { width, height });
        }
        let len = self
            .format
            .expected_size(width, height)
            .map_err(WireError::Geometry)?
            .ok_or_else(|| WireError::Format(self.format.to_string()))?;
        if len > MAX_PAYLOAD_BYTES {
            return Err(WireError::TooLarge(len));
        }
        Ok(len)
    }

    /// `encoded` is the access unit's length for an H.264 stream; a raw
    /// payload's length follows from the header.
    pub(crate) fn encode(
        &self,
        encoding: Encoding,
        encoded: usize,
    ) -> Result<[u8; HEADER_LEN], WireError> {
        let raw = self.payload_len()?;
        let len = match encoding {
            Encoding::Raw => raw,
            Encoding::H264 => {
                self.check_h264()?;
                if !(1..=MAX_PAYLOAD_BYTES).contains(&encoded) {
                    return Err(WireError::EncodedLength(encoded));
                }
                encoded
            }
        };
        let flags = match encoding {
            Encoding::H264 if self.keyframe => KEYFRAME,
            _ => 0,
        };
        let (format, range, matrix) = format_code(self.format)?;
        let mut bytes = [0; HEADER_LEN];
        bytes[..4].copy_from_slice(&[format, range, matrix, flags]);
        bytes[4..8].copy_from_slice(&self.width.to_be_bytes());
        bytes[8..12].copy_from_slice(&self.height.to_be_bytes());
        // Both lengths are bounded far below `u32::MAX`.
        let len = u32::try_from(len).map_err(|_| WireError::TooLarge(len))?;
        bytes[12..16].copy_from_slice(&len.to_be_bytes());
        Ok(bytes)
    }

    /// An H.264 stream carries limited-range BT.709 NV12 only.
    fn check_h264(&self) -> Result<(), WireError> {
        if self.format == prysm_h264::FORMAT {
            Ok(())
        } else {
            Err(WireError::Format(format!(
                "{} in an H.264 stream (limited-range BT.709 NV12 only)",
                self.format
            )))
        }
    }

    /// Validates everything, including the declared length, before the
    /// caller allocates; returns the header and the payload length.
    pub(crate) fn decode(
        bytes: &[u8; HEADER_LEN],
        encoding: Encoding,
    ) -> Result<(Self, usize), WireError> {
        let field = |at: usize| {
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        let flags = bytes[3];
        let allowed = match encoding {
            Encoding::Raw => 0,
            Encoding::H264 => KEYFRAME,
        };
        if flags & !allowed != 0 {
            return Err(WireError::Flags(flags));
        }
        let header = Self {
            width: field(4),
            height: field(8),
            format: parse_format(bytes[0], bytes[1], bytes[2])?,
            keyframe: flags & KEYFRAME != 0,
        };
        let declared = usize::try_from(field(12)).unwrap_or(usize::MAX);
        if declared > MAX_PAYLOAD_BYTES {
            return Err(WireError::TooLarge(declared));
        }
        let expected = header.payload_len()?;
        match encoding {
            Encoding::Raw if declared != expected => Err(WireError::Length {
                expected,
                actual: declared,
            }),
            Encoding::Raw => Ok((header, declared)),
            Encoding::H264 => {
                header.check_h264()?;
                if declared == 0 {
                    return Err(WireError::EncodedLength(0));
                }
                Ok((header, declared))
            }
        }
    }
}

fn yuv_code(range: YuvRange, matrix: YuvStandardMatrix) -> Result<(u8, u8), WireError> {
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
        YuvStandardMatrix::Custom(..) => {
            return Err(WireError::Format("YUV with a custom matrix".into()));
        }
    };
    Ok((range, matrix))
}

fn format_code(format: PixelFormat) -> Result<(u8, u8, u8), WireError> {
    Ok(match format {
        PixelFormat::RGB24 => (1, 0, 0),
        PixelFormat::BGR24 => (2, 0, 0),
        PixelFormat::YUYV { range, matrix } => {
            let (range, matrix) = yuv_code(range, matrix)?;
            (3, range, matrix)
        }
        PixelFormat::NV12 { range, matrix } => {
            let (range, matrix) = yuv_code(range, matrix)?;
            (4, range, matrix)
        }
        PixelFormat::MJPEG => return Err(WireError::Format(format.to_string())),
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
            let bytes = sent.encode(Encoding::Raw, 0).unwrap();
            let (got, len) = Header::decode(&bytes, Encoding::Raw).unwrap();
            assert_eq!(got, sent);
            assert_eq!(Some(len), format.expected_size(320, 180).unwrap());
        }
        // NV12 is half of RGB24.
        let (_, nv12) = Header::decode(
            &header(320, 180, NV12).encode(Encoding::Raw, 0).unwrap(),
            Encoding::Raw,
        )
        .unwrap();
        assert_eq!(nv12, 86_400);
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
            (
                PixelFormat::NV12 {
                    range: YuvRange::Full,
                    matrix: YuvStandardMatrix::Custom(0.2, 0.1),
                },
                2,
            ),
            (NV12, 3),
            (PixelFormat::RGB24, 0),
            (PixelFormat::RGB24, MAX_DIMENSION + 1),
        ] {
            assert!(
                header(width, 2, format).encode(Encoding::Raw, 0).is_err(),
                "{format:?} {width}"
            );
        }
        // Within each dimension limit but over the payload limit.
        assert!(matches!(
            header(4096, 4096, PixelFormat::RGB24).encode(Encoding::Raw, 0),
            Err(WireError::TooLarge(_))
        ));
    }

    #[test]
    fn malformed_headers_are_rejected_before_allocation() {
        let good = header(4, 2, PixelFormat::RGB24)
            .encode(Encoding::Raw, 0)
            .unwrap();
        let with = |at: usize, patch: &[u8]| {
            let mut bytes = good;
            bytes[at..at + patch.len()].copy_from_slice(patch);
            Header::decode(&bytes, Encoding::Raw)
        };
        assert!(matches!(with(0, &[9]), Err(WireError::FormatCode { .. })));
        assert!(matches!(with(1, &[1]), Err(WireError::FormatCode { .. })));
        assert!(matches!(with(3, &[1]), Err(WireError::Flags(1))));
        assert!(matches!(
            with(12, &u32::MAX.to_be_bytes()),
            Err(WireError::TooLarge(_))
        ));
        assert!(matches!(
            with(12, &25u32.to_be_bytes()),
            Err(WireError::Length {
                expected: 24,
                actual: 25
            })
        ));
        assert!(matches!(
            with(4, &u32::MAX.to_be_bytes()),
            Err(WireError::Dimensions { .. })
        ));
        for format in [
            NV12,
            PixelFormat::YUYV {
                range: YuvRange::Limited,
                matrix: YuvStandardMatrix::Bt709,
            },
        ] {
            let mut odd = header(4, 2, format).encode(Encoding::Raw, 0).unwrap();
            odd[4..8].copy_from_slice(&3u32.to_be_bytes());
            assert!(matches!(
                Header::decode(&odd, Encoding::Raw),
                Err(WireError::Geometry(_))
            ));
        }
    }

    fn split(reply: &[u8; REPLY_LEN]) -> ([u8; HELLO_LEN], [u8; HELLO_LEN]) {
        let mut head = [0; HELLO_LEN];
        let mut rest = [0; HELLO_LEN];
        head.copy_from_slice(&reply[..HELLO_LEN]);
        rest.copy_from_slice(&reply[HELLO_LEN..]);
        (head, rest)
    }

    #[test]
    fn hello_and_reply_carry_version_and_request() {
        let sent = hello();
        assert_eq!(sent, *b"PRSM\x03\x00\x00\x00");
        assert_eq!(accept(&sent), Ok(()));
        let request = StreamRequest {
            width: 640,
            height: 280,
            fps: 25,
            include_ui: false,
            encoding: StreamEncoding::H264Software,
        };
        let answer = reply(Ok(&request));
        assert_eq!(
            answer,
            *b"PRSM\x03\x00\x00\x00\x02\x00\x02\x80\x01\x18\x19\x00"
        );
        let (head, rest) = split(&answer);
        assert_eq!(accepted(&head), Ok(()));
        assert_eq!(super::request(&rest), Ok(request));
        for encoding in [StreamEncoding::H264Hardware, StreamEncoding::Raw] {
            let request = StreamRequest {
                encoding,
                include_ui: true,
                ..request
            };
            assert_eq!(super::request(&split(&reply(Ok(&request))).1), Ok(request));
        }

        // A version-2 sender's hello is refused with this end's version.
        let old = *b"PRSM\x02\x01\x00\x00";
        let outcome = accept(&old);
        assert_eq!(outcome, Err(WireError::Version { ours: 3, theirs: 2 }));
        let refused = reply(outcome.as_ref().map(|()| &request));
        assert_eq!(
            refused,
            *b"PRSM\x03\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00"
        );
        assert_eq!(
            accepted(&split(&refused).0),
            Err(WireError::Rejected(UNSUPPORTED_VERSION))
        );
        // A receiver of another version says so in its reply.
        assert_eq!(
            accepted(b"PRSM\x02\x01\x00\x00"),
            Err(WireError::Version { ours: 3, theirs: 2 })
        );
        assert_eq!(accept(b"GET / HT"), Err(WireError::Magic));
        assert_eq!(accepted(b"HTTP/1.1"), Err(WireError::Magic));
    }

    #[test]
    fn malformed_requests_are_refused() {
        let good = split(&reply(Ok(&StreamRequest::default()))).1;
        for (at, value) in [(0, 3), (1, 2), (3, 3), (6, 0), (6, 61), (7, 1)] {
            let mut bad = good;
            bad[at] = value;
            assert!(
                matches!(super::request(&bad), Err(WireError::Request(_))),
                "byte {at} = {value}"
            );
        }
    }

    #[test]
    fn refusals_carry_a_bounded_reason() {
        let bytes = refusal("hardware H.264 encoder cannot start", REFUSED_ENCODING);
        let mut header = [0; HEADER_LEN];
        header.copy_from_slice(&bytes[..HEADER_LEN]);
        assert_eq!(refusal_len(&header), Some(Ok((35, REFUSED_ENCODING))));
        let mut whole = [0; HEADER_LEN];
        whole.copy_from_slice(&refusal("busy", 0)[..HEADER_LEN]);
        assert_eq!(refusal_len(&whole), Some(Ok((4, 0))));
        assert_eq!(&bytes[HEADER_LEN..], b"hardware H.264 encoder cannot start");
        // Long reasons are cut on a character boundary.
        let long = "é".repeat(MAX_REASON_BYTES);
        let bytes = refusal(&long, 0);
        assert_eq!(bytes.len(), HEADER_LEN + MAX_REASON_BYTES);
        assert!(std::str::from_utf8(&bytes[HEADER_LEN..]).is_ok());
        assert_eq!(refusal("", 0).len(), HEADER_LEN + "refused".len());
        // Frames are not refusals; bad refusal headers are errors.
        let frame = header_bytes();
        assert_eq!(refusal_len(&frame), None);
        let mut empty = [0; HEADER_LEN];
        assert_eq!(refusal_len(&empty), Some(Err(WireError::ReasonLength(0))));
        empty[12..16].copy_from_slice(&(MAX_REASON_BYTES as u32 + 1).to_be_bytes());
        assert!(matches!(
            refusal_len(&empty),
            Some(Err(WireError::ReasonLength(_)))
        ));
        let mut flagged = header;
        flagged[3] = 2;
        assert_eq!(refusal_len(&flagged), Some(Err(WireError::Flags(2))));
        let mut ranged = header;
        ranged[1] = 1;
        assert!(matches!(
            refusal_len(&ranged),
            Some(Err(WireError::FormatCode { .. }))
        ));
    }

    fn header_bytes() -> [u8; HEADER_LEN] {
        header(4, 2, PixelFormat::RGB24)
            .encode(Encoding::Raw, 0)
            .unwrap()
    }

    #[test]
    fn h264_headers_carry_length_and_keyframe() {
        let mut key = header(640, 360, NV12);
        key.keyframe = true;
        let bytes = key.encode(Encoding::H264, 1234).unwrap();
        assert_eq!(bytes[3], KEYFRAME);
        assert_eq!(Header::decode(&bytes, Encoding::H264).unwrap(), (key, 1234));
        // Not a keyframe.
        let plain = header(640, 360, NV12);
        let bytes = plain.encode(Encoding::H264, 7).unwrap();
        assert_eq!(bytes[3], 0);
        assert_eq!(Header::decode(&bytes, Encoding::H264).unwrap(), (plain, 7));
        // A raw stream ignores and rejects the flag.
        assert_eq!(key.encode(Encoding::Raw, 0).unwrap()[3], 0);
        let mut flagged = plain.encode(Encoding::Raw, 0).unwrap();
        flagged[3] = KEYFRAME;
        assert_eq!(
            Header::decode(&flagged, Encoding::Raw),
            Err(WireError::Flags(KEYFRAME))
        );
        // Unknown flags, empty or oversized payloads and other formats are refused.
        let mut bits = bytes;
        bits[3] = 2;
        assert_eq!(
            Header::decode(&bits, Encoding::H264),
            Err(WireError::Flags(2))
        );
        assert_eq!(
            plain.encode(Encoding::H264, 0),
            Err(WireError::EncodedLength(0))
        );
        assert!(matches!(
            plain.encode(Encoding::H264, MAX_PAYLOAD_BYTES + 1),
            Err(WireError::EncodedLength(_))
        ));
        let mut empty = bytes;
        empty[12..16].copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(
            Header::decode(&empty, Encoding::H264),
            Err(WireError::EncodedLength(0))
        );
        let mut huge = bytes;
        huge[12..16].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(matches!(
            Header::decode(&huge, Encoding::H264),
            Err(WireError::TooLarge(_))
        ));
        let rgb = header(640, 360, PixelFormat::RGB24);
        assert!(matches!(
            rgb.encode(Encoding::H264, 10),
            Err(WireError::Format(_))
        ));
        let mut rgb_bytes = rgb.encode(Encoding::Raw, 0).unwrap();
        rgb_bytes[12..16].copy_from_slice(&10u32.to_be_bytes());
        assert!(matches!(
            Header::decode(&rgb_bytes, Encoding::H264),
            Err(WireError::Format(_))
        ));
    }

    #[test]
    fn control_codes_round_trip() {
        for control in [Control::Pause, Control::Resume, Control::Keyframe] {
            assert_eq!(Control::from_code(control.code()), Ok(control));
        }
        assert_eq!(Control::from_code(0), Err(WireError::Control(0)));
        assert_eq!(Control::from_code(4), Err(WireError::Control(4)));
    }
}
