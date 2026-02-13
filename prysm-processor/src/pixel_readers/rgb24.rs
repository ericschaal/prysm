use crate::pixel_reader::PixelReader;
use prysm_core::Color;

/// Zero-copy pixel reader for RGB24 format
///
/// RGB24 stores 3 bytes per pixel: [R, G, B]
/// Direct buffer indexing with no allocation or conversion
#[derive(Debug, Clone, Copy, Default)]
pub struct Rgb24Reader;

impl PixelReader for Rgb24Reader {
    fn read_pixel(&self, data: &[u8], x: u32, y: u32, width: u32) -> Color {
        // RGB24: 3 bytes per pixel [R, G, B]
        let pixel_offset = ((y * width + x) * 3) as usize;

        // Bounds check
        if pixel_offset + 2 >= data.len() {
            return Color::black();
        }

        Color::new(
            data[pixel_offset],
            data[pixel_offset + 1],
            data[pixel_offset + 2],
        )
    }
}
