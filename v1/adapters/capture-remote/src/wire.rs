//! The stream hello, its reply and the frame header. See the crate README for
//! the protocol.
use prysm_core::{FrameError, PixelFormat, YuvRange, YuvStandardMatrix};

pub(crate) const MAGIC: [u8; 4] = *b"PRSM";
/// The wire protocol version both ends must speak.
pub const VERSION: u8 = 2;
/// Hello (sender to receiver) and reply (receiver to sender).
pub(crate) const HELLO_LEN: usize = 8;
pub(crate) const HEADER_LEN: usize = 16;
/// Largest accepted width or height.
pub const MAX_DIMENSION: u32 = 4096;
/// Largest accepted payload: one 1080p RGB frame.
pub const MAX_PAYLOAD_BYTES: usize = 1920 * 1080 * 3;

/// How a stream's frame payloads are encoded, fixed per connection by the
/// hello. Code 1 is reserved for H.264 access units.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Encoding {
    /// Uncompressed pixels, exactly the frame's size.
    #[default]
    Raw,
}

impl Encoding {
    const fn code(self) -> u8 {
        match self {
            Self::Raw => 0,
        }
    }

    fn from_code(code: u8) -> Result<Self, WireError> {
        match code {
            0 => Ok(Self::Raw),
            _ => Err(WireError::Encoding(code)),
        }
    }
}

/// Why a stream or frame cannot be encoded, or was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum WireError {
    #[error("not a Prysm frame stream")]
    Magic,
    /// The two ends speak different protocol versions; update both.
    #[error("the peer speaks wire protocol version {theirs}, this end version {ours}")]
    Version { ours: u8, theirs: u8 },
    #[error("unsupported stream encoding {0}")]
    Encoding(u8),
    #[error("receiver rejected the stream with unknown status {0}")]
    Rejected(u8),
    #[error("the receiver did not answer the hello (wire protocol version 1 never does)")]
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

/// The first bytes on a connection: `PRSM`, version, encoding, two zero bytes.
pub(crate) fn hello(encoding: Encoding) -> [u8; HELLO_LEN] {
    let mut bytes = [0; HELLO_LEN];
    bytes[..4].copy_from_slice(&MAGIC);
    bytes[4] = VERSION;
    bytes[5] = encoding.code();
    bytes
}

/// The receiver's check of a hello.
pub(crate) fn accept(bytes: &[u8; HELLO_LEN]) -> Result<Encoding, WireError> {
    if bytes[..4] != MAGIC {
        return Err(WireError::Magic);
    }
    if bytes[4] != VERSION {
        return Err(WireError::Version {
            ours: VERSION,
            theirs: bytes[4],
        });
    }
    Encoding::from_code(bytes[5])
}

/// Reply status codes.
const ACCEPTED: u8 = 0;
const UNSUPPORTED_VERSION: u8 = 1;
const UNSUPPORTED_ENCODING: u8 = 2;

/// The receiver's answer: `PRSM`, its version, a status, two zero bytes; then
/// it closes the connection unless the status accepts the stream.
pub(crate) fn reply(outcome: &Result<Encoding, WireError>) -> [u8; HELLO_LEN] {
    let mut bytes = hello(Encoding::Raw);
    bytes[5] = match outcome {
        Ok(_) => ACCEPTED,
        Err(WireError::Encoding(_)) => UNSUPPORTED_ENCODING,
        Err(_) => UNSUPPORTED_VERSION,
    };
    bytes
}

/// The sender's check of the receiver's reply to `encoding`.
pub(crate) fn accepted(bytes: &[u8; HELLO_LEN], encoding: Encoding) -> Result<(), WireError> {
    if bytes[..4] != MAGIC {
        return Err(WireError::Magic);
    }
    if bytes[4] != VERSION {
        return Err(WireError::Version {
            ours: VERSION,
            theirs: bytes[4],
        });
    }
    match bytes[5] {
        ACCEPTED => Ok(()),
        UNSUPPORTED_ENCODING => Err(WireError::Encoding(encoding.code())),
        status => Err(WireError::Rejected(status)),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Header {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
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

    pub(crate) fn encode(&self, encoding: Encoding) -> Result<[u8; HEADER_LEN], WireError> {
        let len = match encoding {
            Encoding::Raw => self.payload_len()?,
        };
        let (format, range, matrix) = format_code(self.format)?;
        let mut bytes = [0; HEADER_LEN];
        bytes[..4].copy_from_slice(&[format, range, matrix, 0]);
        bytes[4..8].copy_from_slice(&self.width.to_be_bytes());
        bytes[8..12].copy_from_slice(&self.height.to_be_bytes());
        // `payload_len` bounds `len` far below `u32::MAX`.
        let len = u32::try_from(len).map_err(|_| WireError::TooLarge(len))?;
        bytes[12..16].copy_from_slice(&len.to_be_bytes());
        Ok(bytes)
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
        if bytes[3] != 0 {
            return Err(WireError::FormatCode {
                format: bytes[0],
                range: bytes[1],
                matrix: bytes[2],
            });
        }
        let header = Self {
            width: field(4),
            height: field(8),
            format: parse_format(bytes[0], bytes[1], bytes[2])?,
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
            let bytes = sent.encode(Encoding::Raw).unwrap();
            let (got, len) = Header::decode(&bytes, Encoding::Raw).unwrap();
            assert_eq!(got, sent);
            assert_eq!(Some(len), format.expected_size(320, 180).unwrap());
        }
        // NV12 is half of RGB24.
        let (_, nv12) = Header::decode(
            &header(320, 180, NV12).encode(Encoding::Raw).unwrap(),
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
                header(width, 2, format).encode(Encoding::Raw).is_err(),
                "{format:?} {width}"
            );
        }
        // Within each dimension limit but over the payload limit.
        assert!(matches!(
            header(4096, 4096, PixelFormat::RGB24).encode(Encoding::Raw),
            Err(WireError::TooLarge(_))
        ));
    }

    #[test]
    fn malformed_headers_are_rejected_before_allocation() {
        let good = header(4, 2, PixelFormat::RGB24)
            .encode(Encoding::Raw)
            .unwrap();
        let with = |at: usize, patch: &[u8]| {
            let mut bytes = good;
            bytes[at..at + patch.len()].copy_from_slice(patch);
            Header::decode(&bytes, Encoding::Raw)
        };
        assert!(matches!(with(0, &[9]), Err(WireError::FormatCode { .. })));
        assert!(matches!(with(1, &[1]), Err(WireError::FormatCode { .. })));
        assert!(matches!(with(3, &[1]), Err(WireError::FormatCode { .. })));
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
            let mut odd = header(4, 2, format).encode(Encoding::Raw).unwrap();
            odd[4..8].copy_from_slice(&3u32.to_be_bytes());
            assert!(matches!(
                Header::decode(&odd, Encoding::Raw),
                Err(WireError::Geometry(_))
            ));
        }
    }

    #[test]
    fn hello_and_reply_carry_version_and_encoding() {
        let sent = hello(Encoding::Raw);
        assert_eq!(sent, *b"PRSM\x02\x00\x00\x00");
        let outcome = accept(&sent);
        assert_eq!(outcome, Ok(Encoding::Raw));
        assert_eq!(accepted(&reply(&outcome), Encoding::Raw), Ok(()));

        // A version-1 sender opens with its first frame header.
        let old = *b"PRSM\x01\x01\x00\x00";
        let outcome = accept(&old);
        assert_eq!(outcome, Err(WireError::Version { ours: 2, theirs: 1 }));
        assert_eq!(
            accepted(&reply(&outcome), Encoding::Raw),
            Err(WireError::Rejected(UNSUPPORTED_VERSION))
        );
        // A receiver of another version says so in its reply.
        let mut newer = reply(&Ok(Encoding::Raw));
        newer[4] = 3;
        assert_eq!(
            accepted(&newer, Encoding::Raw),
            Err(WireError::Version { ours: 2, theirs: 3 })
        );

        // Reserved encodings (1: H.264) are refused, not misread.
        let mut h264 = sent;
        h264[5] = 1;
        let outcome = accept(&h264);
        assert_eq!(outcome, Err(WireError::Encoding(1)));
        assert_eq!(
            accepted(&reply(&outcome), Encoding::Raw),
            Err(WireError::Encoding(0))
        );
        assert_eq!(accept(b"GET / HT"), Err(WireError::Magic));
        assert_eq!(accepted(b"HTTP/1.1", Encoding::Raw), Err(WireError::Magic));
    }
}
