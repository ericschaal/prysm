use crate::pixel_reader::PixelReader;
use prysm_core::Color;

/// Zero-copy pixel reader for YUYV format with inline YUV→RGB conversion
///
/// YUYV stores 2 pixels in 4 bytes: [Y0 U Y1 V]
/// Each pixel pair shares U and V chrominance values
/// Conversion uses BT.601 fixed-point arithmetic
#[derive(Debug, Clone, Copy, Default)]
pub struct YuyvReader;

impl PixelReader for YuyvReader {
    fn read_pixel(&self, data: &[u8], x: u32, y: u32, width: u32) -> Color {
        // Fixed-point coefficients (multiplied by 1024) - BT.601 standard
        const V_TO_R: i32 = 1437; // 1.402 * 1024
        const U_TO_G: i32 = 352; // 0.344136 * 1024
        const V_TO_G: i32 = 731; // 0.714136 * 1024
        const U_TO_B: i32 = 1814; // 1.772 * 1024

        // YUYV stores 2 pixels in 4 bytes: [Y0 U Y1 V]
        // Each pixel pair shares U and V values

        // Calculate offset for the pixel pair containing this pixel
        let pixel_pair_x = (x / 2) * 2; // Align to even pixel
        let yuyv_offset = ((y * width + pixel_pair_x) * 2) as usize;

        // Bounds check
        if yuyv_offset + 3 >= data.len() {
            return Color::black();
        }

        // Extract Y, U, V values
        let y_value = if x % 2 == 0 {
            data[yuyv_offset] as i32 // Y0 for even pixels
        } else {
            data[yuyv_offset + 2] as i32 // Y1 for odd pixels
        };

        let u = data[yuyv_offset + 1] as i32 - 128;
        let v = data[yuyv_offset + 3] as i32 - 128;

        // Convert YUYV to RGB using BT.601 (inline conversion)
        let r = (y_value + ((v * V_TO_R) >> 10)).clamp(0, 255) as u8;
        let g = (y_value - ((u * U_TO_G + v * V_TO_G) >> 10)).clamp(0, 255) as u8;
        let b = (y_value + ((u * U_TO_B) >> 10)).clamp(0, 255) as u8;

        Color::new(r, g, b)
    }
}
