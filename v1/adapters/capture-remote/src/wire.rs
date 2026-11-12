//! The frame header. See the crate README for the protocol.
use prysm_core::{FrameError, PixelFormat, YuvRange, YuvStandardMatrix};

pub(crate) const MAGIC: [u8; 4] = *b"PRSM";
pub(crate) const VERSION: u8 = 1;
pub(crate) const HEADER_LEN: usize = 20;
/// Largest accepted width or height.
pub const MAX_DIMENSION: u32 = 4096;
/// Largest accepted payload: one 1080p RGB frame.
pub const MAX_PAYLOAD_BYTES: usize = 1920 * 1080 * 3;

/// Why a frame cannot be encoded or a header was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum WireError {
    #[error("not a Prysm frame header")]
    Magic,
    #[error("unsupported protocol version {0}")]
    Version(u8),
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Header {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
}

impl Header {
    /// Checks dimensions and size; returns the exact payload length.
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

    pub(crate) fn encode(&self) -> Result<[u8; HEADER_LEN], WireError> {
        let len = self.payload_len()?;
        let (format, range, matrix) = format_code(self.format)?;
        let mut bytes = [0; HEADER_LEN];
        bytes[..4].copy_from_slice(&MAGIC);
        bytes[4..8].copy_from_slice(&[VERSION, format, range, matrix]);
        bytes[8..12].copy_from_slice(&self.width.to_be_bytes());
        bytes[12..16].copy_from_slice(&self.height.to_be_bytes());
        // `payload_len` bounds `len` far below `u32::MAX`.
        let len = u32::try_from(len).map_err(|_| WireError::TooLarge(len))?;
        bytes[16..20].copy_from_slice(&len.to_be_bytes());
        Ok(bytes)
    }

    /// Validates everything, including the declared length, before the
    /// caller allocates.
    pub(crate) fn decode(bytes: &[u8; HEADER_LEN]) -> Result<Self, WireError> {
        if bytes[..4] != MAGIC {
            return Err(WireError::Magic);
        }
        if bytes[4] != VERSION {
            return Err(WireError::Version(bytes[4]));
        }
        let field = |at: usize| {
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        let header = Self {
            width: field(8),
            height: field(12),
            format: parse_format(bytes[5], bytes[6], bytes[7])?,
        };
        let declared = usize::try_from(field(16)).unwrap_or(usize::MAX);
        if declared > MAX_PAYLOAD_BYTES {
            return Err(WireError::TooLarge(declared));
        }
        let expected = header.payload_len()?;
        if declared != expected {
            return Err(WireError::Length {
                expected,
                actual: declared,
            });
        }
        Ok(header)
    }
}

fn format_code(format: PixelFormat) -> Result<(u8, u8, u8), WireError> {
    Ok(match format {
        PixelFormat::RGB24 => (1, 0, 0),
        PixelFormat::BGR24 => (2, 0, 0),
        PixelFormat::YUYV { range, matrix } => {
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
                    return Err(WireError::Format("YUYV with a custom matrix".into()));
                }
            };
            (3, range, matrix)
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
    Ok(match (format, range, matrix) {
        (1, 0, 0) => PixelFormat::RGB24,
        (2, 0, 0) => PixelFormat::BGR24,
        (3, range, matrix) => PixelFormat::YUYV {
            range: match range {
                1 => YuvRange::Limited,
                2 => YuvRange::Full,
                _ => return Err(unknown),
            },
            matrix: match matrix {
                1 => YuvStandardMatrix::Bt601,
                2 => YuvStandardMatrix::Bt709,
                3 => YuvStandardMatrix::Bt2020,
                4 => YuvStandardMatrix::Smpte240,
                5 => YuvStandardMatrix::Bt470_6,
                6 => YuvStandardMatrix::Fcc,
                _ => return Err(unknown),
            },
        },
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

    #[test]
    fn every_fixed_size_format_round_trips() {
        let yuyv = |range, matrix| PixelFormat::YUYV { range, matrix };
        for format in [
            PixelFormat::RGB24,
            PixelFormat::BGR24,
            yuyv(YuvRange::Limited, YuvStandardMatrix::Bt709),
            yuyv(YuvRange::Full, YuvStandardMatrix::Bt601),
            yuyv(YuvRange::Full, YuvStandardMatrix::Fcc),
        ] {
            let sent = header(320, 180, format);
            assert_eq!(Header::decode(&sent.encode().unwrap()).unwrap(), sent);
        }
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
            (PixelFormat::RGB24, 0),
            (PixelFormat::RGB24, MAX_DIMENSION + 1),
        ] {
            assert!(
                header(width, 2, format).encode().is_err(),
                "{format:?} {width}"
            );
        }
        // Within each dimension limit but over the payload limit.
        assert!(matches!(
            header(4096, 4096, PixelFormat::RGB24).encode(),
            Err(WireError::TooLarge(_))
        ));
    }

    #[test]
    fn malformed_headers_are_rejected_before_allocation() {
        let good = header(4, 2, PixelFormat::RGB24).encode().unwrap();
        let with = |at: usize, patch: &[u8]| {
            let mut bytes = good;
            bytes[at..at + patch.len()].copy_from_slice(patch);
            Header::decode(&bytes)
        };
        assert_eq!(with(0, b"HTTP"), Err(WireError::Magic));
        assert_eq!(with(4, &[2]), Err(WireError::Version(2)));
        assert!(matches!(with(5, &[9]), Err(WireError::FormatCode { .. })));
        assert!(matches!(with(6, &[1]), Err(WireError::FormatCode { .. })));
        assert!(matches!(
            with(16, &u32::MAX.to_be_bytes()),
            Err(WireError::TooLarge(_))
        ));
        assert!(matches!(
            with(16, &25u32.to_be_bytes()),
            Err(WireError::Length {
                expected: 24,
                actual: 25
            })
        ));
        assert!(matches!(
            with(8, &u32::MAX.to_be_bytes()),
            Err(WireError::Dimensions { .. })
        ));
        let odd_yuyv = header(
            4,
            2,
            PixelFormat::YUYV {
                range: YuvRange::Limited,
                matrix: YuvStandardMatrix::Bt709,
            },
        )
        .encode()
        .unwrap();
        let mut odd = odd_yuyv;
        odd[8..12].copy_from_slice(&3u32.to_be_bytes());
        assert!(matches!(Header::decode(&odd), Err(WireError::Geometry(_))));
    }
}
