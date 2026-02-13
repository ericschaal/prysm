use prysm_core::Color;
use std::ops::Range;

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

    /// Read an entire region as `Color` data for SIMD-optimized conversion
    ///
    /// This allows formats like YUYV to use SIMD-optimized conversion for the
    /// entire region, rather than converting pixel-by-pixel.
    ///
    /// The default implementation uses pixel-by-pixel reading via `read_pixel()`.
    /// Formats with specialized bulk conversion (like YUYV) should override this.
    ///
    /// # Arguments
    /// * `data` - Raw frame buffer
    /// * `width` - Frame width in pixels
    /// * `height` - Frame height in pixels (unused by default impl)
    /// * `x_range` - Horizontal range in absolute frame coordinates
    /// * `y_range` - Vertical range in absolute frame coordinates
    ///
    /// # Returns
    /// Vec of colors in row-major order with `x_range.len() × y_range.len()` elements
    fn read_region(
        &self,
        data: &[u8],
        width: u32,
        _height: u32,
        x_range: Range<u32>,
        y_range: Range<u32>,
    ) -> Vec<Color> {
        // Calculate region dimensions (already usize from Range::len)
        let region_width = x_range.len();
        let region_height = y_range.len();

        if region_width == 0 || region_height == 0 {
            return vec![];
        }

        let size = region_width * region_height;
        let mut region = Vec::with_capacity(size);

        for y in 0..region_height {
            for x in 0..region_width {
                // Convert region-relative coordinates to absolute frame coordinates
                let color = self.read_pixel(
                    data,
                    x_range.start + x as u32,
                    y_range.start + y as u32,
                    width,
                );
                region.push(color);
            }
        }

        region
    }
}
