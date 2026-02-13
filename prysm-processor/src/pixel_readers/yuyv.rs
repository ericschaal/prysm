use crate::pixel_reader::PixelReader;
use prysm_core::Color;
use std::ops::Range;

/// Zero-copy pixel reader for YUYV format with inline YUV→RGB conversion
///
/// YUYV stores 2 pixels in 4 bytes: [Y0 U Y1 V]
/// Each pixel pair shares U and V chrominance values
/// Conversion uses BT.601 standard (same as full-frame conversion)
///
/// # Optimization
/// Implements `read_region()` to use SIMD-optimized conversion for entire regions,
/// which is ~10x faster than pixel-by-pixel conversion for edge sampling.
#[derive(Debug, Clone, Copy, Default)]
pub struct YuyvReader;

impl PixelReader for YuyvReader {
    fn read_pixel(&self, data: &[u8], x: u32, y: u32, width: u32) -> Color {
        let (r, g, b) = prysm_capture::yuyv::yuyv_pixel_to_rgb(data, x, y, width);
        Color::new(r, g, b)
    }

    fn read_region(
        &self,
        data: &[u8],
        width: u32,
        _height: u32,
        x_range: Range<u32>,
        y_range: Range<u32>,
    ) -> Vec<Color> {
        let region_width = x_range.len();
        let region_height = y_range.len();

        if region_width == 0 || region_height == 0 {
            return vec![];
        }

        // YUYV must align to even pixel boundaries (2 pixels per YUYV pair)
        let x_start_aligned = (x_range.start / 2) * 2;
        let x_end_aligned = x_range.end.div_ceil(2) * 2; // Round up to even
        let aligned_width = (x_end_aligned - x_start_aligned) as usize;

        // Extract YUYV sub-region from frame
        let mut yuyv_region = Vec::with_capacity(aligned_width * 2 * region_height);

        for y in y_range {
            let row_start = (y * width * 2 + x_start_aligned * 2) as usize;
            let row_end = row_start + aligned_width * 2;

            // Bounds check
            if row_end > data.len() {
                return vec![];
            }

            yuyv_region.extend_from_slice(&data[row_start..row_end]);
        }

        // Convert region using SIMD-optimized function
        let rgb_region =
            prysm_capture::yuyv::yuyv_to_rgb(&yuyv_region, aligned_width, region_height);

        // Convert RGB bytes to Color structs
        let x_offset = (x_range.start - x_start_aligned) as usize;
        let mut colors = Vec::with_capacity(region_width * region_height);

        for y in 0..region_height {
            for x in 0..region_width {
                let src_x = x + x_offset;
                let offset = (y * aligned_width + src_x) * 3;

                if offset + 2 < rgb_region.len() {
                    colors.push(Color::new(
                        rgb_region[offset],
                        rgb_region[offset + 1],
                        rgb_region[offset + 2],
                    ));
                } else {
                    colors.push(Color::black());
                }
            }
        }

        colors
    }
}
