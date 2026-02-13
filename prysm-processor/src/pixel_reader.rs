use prysm_core::Color;

/// `PixelReader` abstracts pixel format differences for zero-copy pixel access
///
/// Implementations convert (x, y) coordinates to RGB colors by reading directly
/// from the raw frame buffer without allocation.
pub trait PixelReader {
    /// Read a pixel at (x, y) coordinates from the raw frame data
    ///
    /// # Arguments
    /// * `data` - Raw frame buffer (zero-copy reference)
    /// * `x` - X coordinate of the pixel
    /// * `y` - Y coordinate of the pixel
    /// * `width` - Frame width in pixels
    ///
    /// # Returns
    /// Stack-allocated Color (no heap allocation)
    fn read_pixel(&self, data: &[u8], x: u32, y: u32, width: u32) -> Color;
}
