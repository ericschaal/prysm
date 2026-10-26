//! Frame storage, pixel formats, and capture-payload validation.
use crate::{YuvRange, YuvStandardMatrix};
use std::{
    fmt::{Display, Formatter},
    sync::Arc,
};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    RGB24, // 3 bytes per pixel
    BGR24, // 3 bytes per pixel
    #[default]
    YUYV, // 2 bytes per pixel (4:2:2 subsampling)
    MJPEG, // Variable size (future support)
}

impl Display for PixelFormat {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            PixelFormat::RGB24 => f.write_str("RGB24"),
            PixelFormat::BGR24 => f.write_str("BGR24"),
            PixelFormat::YUYV => f.write_str("YUYV"),
            PixelFormat::MJPEG => f.write_str("JPEG"),
        }
    }
}

impl PixelFormat {
    /// Returns the bytes per pixel for this format.
    /// Returns None for variable-size formats like MJPEG.
    pub fn bytes_per_pixel(&self) -> Option<usize> {
        match self {
            PixelFormat::RGB24 | PixelFormat::BGR24 => Some(3),
            PixelFormat::YUYV => Some(2),
            PixelFormat::MJPEG => None,
        }
    }

    /// Returns the expected buffer size for a frame with the given dimensions.
    /// Returns None for variable-size formats like MJPEG.
    pub fn expected_size(&self, width: u32, height: u32) -> Option<usize> {
        self.bytes_per_pixel()
            .map(|bpp| (width as usize) * (height as usize) * bpp)
    }
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub data: Arc<Vec<u8>>,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    /// YUYV color metadata; ignored for RGB and compressed formats.
    pub yuv_range: YuvRange,
    pub yuv_matrix: YuvStandardMatrix,
}

impl Frame {
    /// Validate a capture payload and remove row padding before constructing a frame.
    #[cfg(any(target_os = "linux", test))]
    pub(crate) fn from_strided_buffer(
        data: &[u8],
        width: u32,
        height: u32,
        format: PixelFormat,
        stride: usize,
        corrupted: bool,
    ) -> Option<Self> {
        if corrupted
            || width == 0
            || height == 0
            || (format == PixelFormat::YUYV && !width.is_multiple_of(2))
        {
            return None;
        }
        let row_size = (width as usize).checked_mul(format.bytes_per_pixel()?)?;
        let required = stride
            .checked_mul(height as usize - 1)?
            .checked_add(row_size)?;
        if stride < row_size || data.len() < required {
            return None;
        }
        let mut packed = Vec::with_capacity(row_size.checked_mul(height as usize)?);
        for row in 0..height as usize {
            packed.extend_from_slice(&data[row * stride..row * stride + row_size]);
        }
        Some(Self::new(packed, width, height, format))
    }

    /// Creates a new frame with the given data, dimensions, and pixel format.
    /// YUYV defaults to full-range BT.601; capturers set the negotiated metadata.
    ///
    /// # Panics
    /// Panics if the data size doesn't match the expected size for the given format and dimensions
    /// (except for variable-size formats like MJPEG), or if YUYV width is odd.
    pub fn new(data: Vec<u8>, width: u32, height: u32, format: PixelFormat) -> Self {
        assert!(
            format != PixelFormat::YUYV || width.is_multiple_of(2),
            "YUYV width must be even"
        );
        // Validate buffer size for fixed-size formats
        if let Some(expected) = format.expected_size(width, height) {
            assert_eq!(
                data.len(),
                expected,
                "Frame data size mismatch: expected {} bytes for {}x{} {:?}, got {}",
                expected,
                width,
                height,
                format,
                data.len()
            );
        }

        Self {
            data: Arc::new(data),
            width,
            height,
            format,
            yuv_range: YuvRange::Full,
            yuv_matrix: YuvStandardMatrix::Bt601,
        }
    }

    /// Create a frame filled with the specified byte value
    #[must_use]
    pub fn fill(value: u8, width: u32, height: u32, format: PixelFormat) -> Self {
        let expected_size = format.expected_size(width, height).unwrap();
        let data = vec![value; expected_size];
        Self::new(data, width, height, format)
    }

    /// Create a black RGB24 placeholder, including for odd-width video input.
    #[must_use]
    pub fn dummy(width: u32, height: u32) -> Self {
        Self::fill(0, width, height, PixelFormat::RGB24)
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

    #[test]
    fn dummy_frame_supports_odd_rgb_dimensions() {
        let frame = Frame::dummy(3, 2);
        assert_eq!(
            (frame.width, frame.height, frame.format),
            (3, 2, PixelFormat::RGB24)
        );
        assert_eq!(frame.as_slice(), &[0; 18]);
    }

    #[test]
    fn strided_capture_rejects_corruption_and_short_payloads() {
        let bytes = [16, 128, 235, 128, 0, 0, 16, 128, 235, 128];
        let frame = Frame::from_strided_buffer(&bytes, 2, 2, PixelFormat::YUYV, 6, false).unwrap();
        assert_eq!(frame.as_slice(), [16, 128, 235, 128].repeat(2));
        for (data, stride, corrupted) in [
            (&bytes[..], 6, true),
            (&bytes[..9], 6, false),
            (&bytes[..], 3, false),
            (&bytes[..], usize::MAX, false),
        ] {
            assert!(
                Frame::from_strided_buffer(data, 2, 2, PixelFormat::YUYV, stride, corrupted)
                    .is_none()
            );
        }
        assert!(Frame::from_strided_buffer(&bytes, 1, 2, PixelFormat::YUYV, 6, false).is_none());
        assert!(Frame::from_strided_buffer(&bytes, 2, 0, PixelFormat::YUYV, 6, false).is_none());
    }

    #[test]
    #[should_panic(expected = "YUYV width must be even")]
    fn odd_yuyv_width_is_rejected() {
        Frame::new(vec![128; 6], 3, 1, PixelFormat::YUYV);
    }
}
