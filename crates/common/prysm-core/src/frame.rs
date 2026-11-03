//! Frame storage, pixel formats, and capture-payload validation.
use crate::{YuvRange, YuvStandardMatrix};
use std::{
    fmt::{Display, Formatter},
    sync::Arc,
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
    MJPEG, // Variable size (future support)
}

impl Display for PixelFormat {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            PixelFormat::RGB24 => f.write_str("RGB24"),
            PixelFormat::BGR24 => f.write_str("BGR24"),
            PixelFormat::YUYV { .. } => f.write_str("YUYV"),
            PixelFormat::MJPEG => f.write_str("JPEG"),
        }
    }
}

/// Invalid frame geometry, payload length, or fixed-size allocation request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    OddYuyvWidth(u32),
    SizeOverflow,
    BufferLength { expected: usize, actual: usize },
    VariableSizeFormat,
}

impl Display for FrameError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OddYuyvWidth(width) => write!(f, "YUYV width must be even, got {width}"),
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
    /// Returns the bytes per pixel for this format.
    /// Returns None for variable-size formats like MJPEG.
    #[must_use]
    pub fn bytes_per_pixel(&self) -> Option<usize> {
        match self {
            PixelFormat::RGB24 | PixelFormat::BGR24 => Some(3),
            PixelFormat::YUYV { .. } => Some(2),
            PixelFormat::MJPEG => None,
        }
    }

    /// Returns the expected buffer size, or `Ok(None)` for variable-size formats like MJPEG.
    /// Returns an error for odd YUYV widths or dimensions exceeding the supported buffer size.
    pub fn expected_size(&self, width: u32, height: u32) -> Result<Option<usize>, FrameError> {
        if matches!(self, Self::YUYV { .. }) && !width.is_multiple_of(2) {
            return Err(FrameError::OddYuyvWidth(width));
        }
        let Some(bpp) = self.bytes_per_pixel() else {
            return Ok(None);
        };
        let size = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(bpp))
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
}

/// Producer-owned storage for up to four reusable pixel buffers.
/// Consumers keep ordinary immutable [`Frame`]s. A busy buffer is never copied or
/// overwritten: if all slots are held, one slot is replaced with fresh storage.
/// The displaced storage lives until its consumers release it.
#[derive(Debug, Default)]
pub struct FramePool {
    buffers: [Option<Arc<Vec<u8>>>; 4],
}

/// Exclusive access to a validated frame buffer before publication.
/// Fill the entire slice before calling [`Self::finish`]. Reused bytes are not cleared.
/// Dropping a writer after a failed or cancelled read publishes nothing.
pub struct FrameWriter<'a> {
    data: &'a mut Arc<Vec<u8>>,
    width: u32,
    height: u32,
    format: PixelFormat,
}

impl FrameWriter<'_> {
    /// The fixed-length destination for capture, reading or conversion.
    ///
    /// # Panics
    /// Panics if the internal exclusive-ownership invariant is violated.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        Arc::get_mut(self.data).expect("writer has exclusive access to pixels")
    }

    /// Publish the completed pixels without copying or allocating.
    #[must_use]
    pub fn finish(self) -> Frame {
        Frame {
            data: Arc::clone(self.data),
            width: self.width,
            height: self.height,
            format: self.format,
        }
    }
}

impl FramePool {
    /// Acquire initialized storage, reusing an unshared allocation when possible.
    /// Geometry is validated before allocating. Only fixed-size formats are supported.
    ///
    /// # Panics
    /// Panics if the internal exclusive-ownership invariant is violated.
    pub fn acquire(
        &mut self,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Result<FrameWriter<'_>, FrameError> {
        let size = format
            .expected_size(width, height)?
            .ok_or(FrameError::VariableSizeFormat)?;
        let index = self.buffers.iter_mut().position(|slot| {
            slot.as_mut()
                .is_none_or(|data| Arc::get_mut(data).is_some())
        });
        // Replacing a busy slot releases only the pool's reference, never a consumer's.
        let index = index.unwrap_or_else(|| {
            self.buffers[0] = None;
            0
        });
        let data = self.buffers[index].get_or_insert_with(|| Arc::new(Vec::new()));
        Arc::get_mut(data)
            .expect("selected buffer is unshared")
            .resize(size, 0);
        Ok(FrameWriter {
            data,
            width,
            height,
            format,
        })
    }

    /// Validate a capture payload and remove row padding before constructing a frame.
    #[must_use]
    pub fn copy_from_strided(
        &mut self,
        data: &[u8],
        width: u32,
        height: u32,
        format: PixelFormat,
        stride: usize,
        corrupted: bool,
    ) -> Option<Frame> {
        if corrupted || width == 0 || height == 0 {
            return None;
        }
        format.expected_size(width, height).ok()??;
        let row_size = (width as usize).checked_mul(format.bytes_per_pixel()?)?;
        let required = stride
            .checked_mul(height as usize - 1)?
            .checked_add(row_size)?;
        if stride < row_size || data.len() < required {
            return None;
        }
        let mut frame = self.acquire(width, height, format).ok()?;
        if stride == row_size {
            frame.as_mut_slice().copy_from_slice(&data[..required]);
            return Some(frame.finish());
        }
        for (row, target) in frame.as_mut_slice().chunks_exact_mut(row_size).enumerate() {
            target.copy_from_slice(&data[row * stride..row * stride + row_size]);
        }
        Some(frame.finish())
    }
}

impl Frame {
    /// Validate and copy one capture payload, removing row padding.
    /// Repeated capture should use [`FramePool::copy_from_strided`] to reuse storage.
    #[must_use]
    pub fn from_strided_buffer(
        data: &[u8],
        width: u32,
        height: u32,
        format: PixelFormat,
        stride: usize,
        corrupted: bool,
    ) -> Option<Self> {
        FramePool::default().copy_from_strided(data, width, height, format, stride, corrupted)
    }

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

    fn filled(pool: &mut FramePool, value: u8) -> Frame {
        let mut writer = pool.acquire(4, 2, PixelFormat::RGB24).unwrap();
        writer.as_mut_slice().fill(value);
        writer.finish()
    }

    #[test]
    fn pool_reuses_storage_only_after_every_consumer_releases_it() {
        let mut pool = FramePool::default();
        let first = filled(&mut pool, 1);
        let pixels = first.as_slice().as_ptr();
        let allocation = Arc::as_ptr(&first.data);
        let retained = first.clone();
        drop(first);
        let second = filled(&mut pool, 2);
        assert_ne!(second.as_slice().as_ptr(), pixels);
        assert_eq!(retained.as_slice(), &[1; 24]);
        std::thread::spawn(move || drop(retained)).join().unwrap();
        let third = filled(&mut pool, 3);
        assert_eq!(third.as_slice().as_ptr(), pixels);
        assert_eq!(Arc::as_ptr(&third.data), allocation);
        assert_eq!(second.as_slice(), &[2; 24]);
    }

    #[test]
    fn saturation_keeps_all_published_frames_intact_and_cache_bounded() {
        let mut pool = FramePool::default();
        let held: Vec<_> = (0..20).map(|value| filled(&mut pool, value)).collect();
        assert_eq!(pool.buffers.iter().flatten().count(), 4);
        for (value, frame) in (0..20).zip(&held) {
            assert_eq!(frame.as_slice(), &[value; 24]);
        }
        drop(held);
        let frame = filled(&mut pool, 42);
        let pixels = frame.as_slice().as_ptr();
        drop(frame);
        let frame = filled(&mut pool, 43);
        assert_eq!(frame.as_slice().as_ptr(), pixels);
        drop(pool);
        assert_eq!(frame.as_slice(), &[43; 24]);
    }

    #[test]
    fn failed_write_resize_and_format_changes_do_not_publish_stale_pixels() {
        let mut pool = FramePool::default();
        let retained = filled(&mut pool, 1);
        {
            let mut incomplete = pool.acquire(4, 2, PixelFormat::RGB24).unwrap();
            incomplete.as_mut_slice()[..2].fill(99);
        }
        let mut resized = pool.acquire(8, 2, PixelFormat::BGR24).unwrap();
        resized.as_mut_slice().fill(2);
        let resized = resized.finish();
        assert_eq!(resized.as_slice(), &[2; 48]);
        assert_eq!(resized.format(), PixelFormat::BGR24);
        assert_eq!(retained.as_slice(), &[1; 24]);
        drop(resized);
        let mut smaller = pool.acquire(2, 2, PixelFormat::RGB24).unwrap();
        smaller.as_mut_slice().fill(3);
        assert_eq!(smaller.finish().as_slice(), &[3; 12]);
    }

    #[test]
    fn pool_rejects_invalid_geometry_before_allocating() {
        let mut pool = FramePool::default();
        assert!(
            pool.acquire(u32::MAX, u32::MAX, PixelFormat::RGB24)
                .is_err()
        );
        assert!(pool.acquire(2, 2, PixelFormat::MJPEG).is_err());
        let format = PixelFormat::YUYV {
            range: YuvRange::Full,
            matrix: YuvStandardMatrix::Bt709,
        };
        assert!(pool.acquire(3, 2, format).is_err());
        assert!(pool.buffers.iter().all(Option::is_none));
    }

    #[test]
    fn strided_capture_reuses_packed_storage_and_validates_each_payload() {
        let mut pool = FramePool::default();
        let frame = pool
            .copy_from_strided(&[1, 2, 3, 99, 4, 5, 6], 1, 2, PixelFormat::RGB24, 4, false)
            .unwrap();
        let pixels = frame.as_slice().as_ptr();
        assert_eq!(frame.as_slice(), &[1, 2, 3, 4, 5, 6]);
        drop(frame);
        assert!(
            pool.copy_from_strided(&[7; 6], 1, 2, PixelFormat::RGB24, 4, false)
                .is_none()
        );
        let frame = pool
            .copy_from_strided(&[8; 6], 1, 2, PixelFormat::RGB24, 3, false)
            .unwrap();
        assert_eq!(frame.as_slice().as_ptr(), pixels);
        assert_eq!(frame.as_slice(), &[8; 6]);
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
        let frame = Frame::from_strided_buffer(&bytes, 2, 2, format, 6, false).unwrap();
        assert_eq!(frame.format(), format);
        assert_eq!(frame.as_slice(), [16, 128, 235, 128].repeat(2));
        for (data, stride, corrupted) in [
            (&bytes[..], 6, true),
            (&bytes[..9], 6, false),
            (&bytes[..], 3, false),
            (&bytes[..], usize::MAX, false),
        ] {
            assert!(Frame::from_strided_buffer(data, 2, 2, format, stride, corrupted).is_none());
        }
        assert!(Frame::from_strided_buffer(&bytes, 1, 2, format, 6, false).is_none());
        assert!(Frame::from_strided_buffer(&bytes, 2, 0, format, 6, false).is_none());
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
