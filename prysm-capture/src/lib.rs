use futures::Stream;
use std::fmt::{Display, Formatter};
use std::sync::Arc;

pub mod yuyv;

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
}

impl Frame {
    /// Creates a new frame with the given data, dimensions, and pixel format.
    ///
    /// # Panics
    /// Panics if the data size doesn't match the expected size for the given format and dimensions
    /// (except for variable-size formats like MJPEG).
    pub fn new(data: Vec<u8>, width: u32, height: u32, format: PixelFormat) -> Self {
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
        }
    }

    /// Create a frame filled with the specified byte value
    #[must_use]
    pub fn fill(value: u8, width: u32, height: u32, format: PixelFormat) -> Self {
        let expected_size = format.expected_size(width, height).unwrap();
        let data = vec![value; expected_size];
        Self::new(data, width, height, format)
    }

    /// Create a dummy frame (zero-filled, which appears green in YUYV)
    #[must_use]
    pub fn dummy(width: u32, height: u32) -> Self {
        Self::fill(0, width, height, PixelFormat::YUYV)
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

pub trait PrysmCapturer {
    fn into_stream(self, width: u32, height: u32) -> impl Stream<Item = Frame> + Send + 'static
    where
        Self: Sized + Send + 'static;
}
