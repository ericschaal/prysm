//! Immutable frame storage, pixel formats, and payload validation.
mod pool;

pub use pool::{FramePool, FramePoolStats, FrameWriter};

use crate::{YuvRange, YuvStandardMatrix};
use std::{
    fmt::{Display, Formatter},
    sync::Arc,
    time::Instant,
};

/// Pixel encoding, including color metadata only where it applies.
/// YUYV requires both range and matrix:
/// ```compile_fail
/// use prysm_core::{PixelFormat, YuvRange};
/// let format = PixelFormat::YUYV { range: YuvRange::Limited };
/// ```
/// RGB cannot carry YUV metadata:
/// ```compile_fail
/// use prysm_core::{PixelFormat, YuvRange, YuvStandardMatrix};
/// let format = PixelFormat::RGB24 { range: YuvRange::Limited, matrix: YuvStandardMatrix::Bt709 };
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PixelFormat {
    RGB24, // 3 bytes per pixel
    BGR24, // 3 bytes per pixel
    /// Packed 4:2:2 pixels with the color encoding supplied by the source.
    YUYV {
        range: YuvRange,
        matrix: YuvStandardMatrix,
    },
    /// Planar 4:2:0: a full-size Y plane, then one interleaved UV plane at
    /// half width and height. Width and height are even. The color encoding
    /// is supplied by the source (LG's capture tap: limited BT.709).
    NV12 {
        range: YuvRange,
        matrix: YuvStandardMatrix,
    },
    MJPEG, // Variable size (future support)
}

impl Display for PixelFormat {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            PixelFormat::RGB24 => f.write_str("RGB24"),
            PixelFormat::BGR24 => f.write_str("BGR24"),
            PixelFormat::YUYV { .. } => f.write_str("YUYV"),
            PixelFormat::NV12 { .. } => f.write_str("NV12"),
            PixelFormat::MJPEG => f.write_str("JPEG"),
        }
    }
}

/// Invalid frame geometry, payload length, or fixed-size allocation request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    OddYuyvWidth(u32),
    OddNv12Dimensions { width: u32, height: u32 },
    SizeOverflow,
    BufferLength { expected: usize, actual: usize },
    VariableSizeFormat,
}

impl Display for FrameError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OddYuyvWidth(width) => write!(f, "YUYV width must be even, got {width}"),
            Self::OddNv12Dimensions { width, height } => {
                write!(
                    f,
                    "NV12 width and height must be even, got {width}x{height}"
                )
            }
            Self::SizeOverflow => f.write_str("frame dimensions exceed the supported buffer size"),
            Self::BufferLength { expected, actual } => {
                write!(f, "frame requires {expected} bytes, got {actual}")
            }
            Self::VariableSizeFormat => f.write_str("cannot fill a variable-size pixel format"),
        }
    }
}

impl std::error::Error for FrameError {}

impl PixelFormat {
    /// Returns the bytes per pixel for packed formats.
    /// Returns None for planar NV12 (1.5 bytes per pixel) and for
    /// variable-size formats like MJPEG.
    fn bytes_per_pixel(&self) -> Option<usize> {
        match self {
            PixelFormat::RGB24 | PixelFormat::BGR24 => Some(3),
            PixelFormat::YUYV { .. } => Some(2),
            PixelFormat::NV12 { .. } | PixelFormat::MJPEG => None,
        }
    }

    /// Bytes per stored row and the number of stored rows, counting both NV12
    /// planes (each NV12 row, luma or chroma, is `width` bytes).
    /// `None` for variable-size formats.
    fn rows(&self, width: u32, height: u32) -> Option<(usize, usize)> {
        let (width, height) = (width as usize, height as usize);
        match self {
            Self::NV12 { .. } => Some((width, height + height / 2)),
            _ => Some((width.checked_mul(self.bytes_per_pixel()?)?, height)),
        }
    }

    /// Returns the expected buffer size, or `Ok(None)` for variable-size formats like MJPEG.
    /// Returns an error for odd YUYV widths, odd NV12 dimensions or dimensions
    /// exceeding the supported buffer size.
    pub fn expected_size(&self, width: u32, height: u32) -> Result<Option<usize>, FrameError> {
        match self {
            Self::YUYV { .. } if !width.is_multiple_of(2) => {
                return Err(FrameError::OddYuyvWidth(width));
            }
            Self::NV12 { .. } if !width.is_multiple_of(2) || !height.is_multiple_of(2) => {
                return Err(FrameError::OddNv12Dimensions { width, height });
            }
            Self::MJPEG => return Ok(None),
            _ => {}
        }
        let size = self
            .rows(width, height)
            .and_then(|(row, rows)| row.checked_mul(rows))
            .filter(|&bytes| isize::try_from(bytes).is_ok())
            .ok_or(FrameError::SizeOverflow)?;
        Ok(Some(size))
    }
}

/// A shared pixel buffer whose dimensions and format cannot change independently.
///
/// ```compile_fail
/// let mut frame = prysm_core::Frame::dummy(2, 2).unwrap();
/// frame.width = 3;
/// ```
/// ```compile_fail
/// let mut frame = prysm_core::Frame::dummy(2, 2).unwrap();
/// frame.height = 3;
/// ```
/// ```compile_fail
/// let mut frame = prysm_core::Frame::dummy(2, 2).unwrap();
/// frame.format = prysm_core::PixelFormat::RGB24;
/// ```
/// ```compile_fail
/// let mut frame = prysm_core::Frame::dummy(2, 2).unwrap();
/// frame.data = std::sync::Arc::new(Vec::new());
/// ```
#[derive(Debug, Clone)]
pub struct Frame {
    data: Arc<Vec<u8>>,
    width: u32,
    height: u32,
    format: PixelFormat,
    captured: Option<Instant>,
}

impl Frame {
    /// Creates a new frame with the given data, dimensions, and pixel format.
    /// YUYV color metadata is required by its format variant.
    ///
    /// # Errors
    /// Returns an error for odd YUYV widths, buffer-size overflow, or a payload
    /// whose length does not match the fixed-size format. Zero-sized frames remain valid.
    pub fn new(
        data: Vec<u8>,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<Self, FrameError> {
        if let Some(expected) = format.expected_size(width, height)?
            && data.len() != expected
        {
            return Err(FrameError::BufferLength {
                expected,
                actual: data.len(),
            });
        }
        Ok(Self {
            data: Arc::new(data),
            width,
            height,
            format,
            captured: None,
        })
    }

    /// Create a frame filled with the specified byte value.
    /// Returns an error for invalid dimensions or a variable-size format such as MJPEG.
    pub fn fill(
        value: u8,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<Self, FrameError> {
        let expected_size = format
            .expected_size(width, height)?
            .ok_or(FrameError::VariableSizeFormat)?;
        Self::new(vec![value; expected_size], width, height, format)
    }

    /// Create a black RGB24 placeholder, including for odd-width video input.
    /// Returns an error if the dimensions overflow the supported buffer size.
    pub fn dummy(width: u32, height: u32) -> Result<Self, FrameError> {
        Self::fill(0, width, height, PixelFormat::RGB24)
    }

    /// Width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Encoding of the tightly packed pixel buffer.
    #[must_use]
    pub const fn format(&self) -> PixelFormat {
        self.format
    }

    /// When the source captured this frame, on [`Instant`]'s clock; `None`
    /// when the source does not say.
    #[must_use]
    pub const fn captured(&self) -> Option<Instant> {
        self.captured
    }

    /// This frame, captured at `captured`.
    #[must_use]
    pub const fn with_captured(mut self, captured: Option<Instant>) -> Self {
        self.captured = captured;
        self
    }

    /// The Y plane (`width` bytes per row) and the interleaved UV plane
    /// (`width` bytes per row, half as many rows) of an NV12 frame.
    /// `None` for every other format.
    #[must_use]
    pub fn nv12_planes(&self) -> Option<(&[u8], &[u8])> {
        matches!(self.format, PixelFormat::NV12 { .. }).then(|| {
            self.data
                .split_at(self.width as usize * self.height as usize)
        })
    }

    /// Returns a slice view of the frame data.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }

    /// Returns the size of the frame data in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Returns true if the frame data is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strided(
        data: &[u8],
        width: u32,
        height: u32,
        format: PixelFormat,
        stride: usize,
        corrupted: bool,
    ) -> Option<Frame> {
        FramePool::default().copy_from_strided(data, width, height, format, stride, corrupted)
    }

    #[test]
    fn dummy_frame_supports_odd_rgb_dimensions() {
        let frame = Frame::dummy(3, 2).unwrap();
        assert_eq!(
            (frame.width, frame.height, frame.format),
            (3, 2, PixelFormat::RGB24)
        );
        assert_eq!(frame.as_slice(), &[0; 18]);
    }

    #[test]
    fn strided_capture_rejects_corruption_and_short_payloads() {
        let format = PixelFormat::YUYV {
            range: YuvRange::Limited,
            matrix: YuvStandardMatrix::Bt709,
        };
        let bytes = [16, 128, 235, 128, 0, 0, 16, 128, 235, 128];
        let frame = strided(&bytes, 2, 2, format, 6, false).unwrap();
        assert_eq!(frame.format(), format);
        assert_eq!(frame.as_slice(), [16, 128, 235, 128].repeat(2));
        for (data, stride, corrupted) in [
            (&bytes[..], 6, true),
            (&bytes[..9], 6, false),
            (&bytes[..], 3, false),
            (&bytes[..], usize::MAX, false),
        ] {
            assert!(strided(data, 2, 2, format, stride, corrupted).is_none());
        }
        assert!(strided(&bytes, 1, 2, format, 6, false).is_none());
        assert!(strided(&bytes, 2, 0, format, 6, false).is_none());
    }

    #[test]
    fn nv12_holds_a_luma_plane_then_half_height_chroma() {
        let format = PixelFormat::NV12 {
            range: YuvRange::Limited,
            matrix: YuvStandardMatrix::Bt709,
        };
        assert_eq!(format.expected_size(4, 2), Ok(Some(12)));
        assert_eq!(format.to_string(), "NV12");
        for (width, height) in [(3, 2), (4, 3)] {
            assert_eq!(
                format.expected_size(width, height),
                Err(FrameError::OddNv12Dimensions { width, height })
            );
        }
        let frame = Frame::new((0..12).collect(), 4, 2, format).unwrap();
        let (y, uv) = frame.nv12_planes().unwrap();
        assert_eq!(
            (y, uv),
            (&[0, 1, 2, 3, 4, 5, 6, 7][..], &[8, 9, 10, 11][..])
        );
        assert!(Frame::dummy(2, 2).unwrap().nv12_planes().is_none());
        // Stride padding is removed from both planes.
        let padded = [1, 2, 9, 3, 4, 9, 5, 6];
        let frame = strided(&padded, 2, 2, format, 3, false).unwrap();
        assert_eq!(frame.as_slice(), [1, 2, 3, 4, 5, 6]);
        assert!(strided(&padded[..7], 2, 2, format, 3, false).is_none());
        assert!(Frame::fill(0, 2, 3, format).is_err());
    }

    #[test]
    fn invalid_frames_return_typed_errors() {
        let yuyv = PixelFormat::YUYV {
            range: YuvRange::Full,
            matrix: YuvStandardMatrix::Bt601,
        };
        assert_eq!(
            Frame::new(vec![128; 6], 3, 1, yuyv).unwrap_err(),
            FrameError::OddYuyvWidth(3)
        );
        for length in [5, 7] {
            assert_eq!(
                Frame::new(vec![0; length], 2, 1, PixelFormat::RGB24).unwrap_err(),
                FrameError::BufferLength {
                    expected: 6,
                    actual: length
                }
            );
        }
        assert_eq!(
            Frame::new(vec![], u32::MAX, u32::MAX, PixelFormat::RGB24).unwrap_err(),
            FrameError::SizeOverflow
        );
        assert_eq!(
            Frame::dummy(u32::MAX, u32::MAX).unwrap_err(),
            FrameError::SizeOverflow
        );
        assert_eq!(
            Frame::fill(0, 3, 1, yuyv).unwrap_err(),
            FrameError::OddYuyvWidth(3)
        );
        assert_eq!(
            Frame::fill(0, 2, 2, PixelFormat::MJPEG).unwrap_err(),
            FrameError::VariableSizeFormat
        );
        assert!(
            Frame::new(vec![], 0, 0, PixelFormat::RGB24)
                .unwrap()
                .is_empty()
        );
        assert!(Frame::new(vec![1], 2, 2, PixelFormat::MJPEG).is_ok());
    }
}
