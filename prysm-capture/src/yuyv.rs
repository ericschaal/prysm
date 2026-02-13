use yuv::{YuvPackedImage, YuvRange, YuvStandardMatrix, yuyv422_to_rgb};

/// Convert a single YUYV pixel to RGB using BT.601 standard
///
/// This function extracts and converts one pixel from YUYV format.
/// YUYV stores 2 pixels in 4 bytes: [Y0 U Y1 V], where U and V are shared.
///
/// # Arguments
/// * `yuyv_data` - Raw YUYV buffer
/// * `x` - X coordinate of pixel
/// * `y` - Y coordinate of pixel
/// * `width` - Frame width in pixels
///
/// # Returns
/// RGB tuple (r, g, b) with values 0-255
///
/// # Implementation
/// Uses BT.601 fixed-point arithmetic for consistency with full-frame conversion.
pub fn yuyv_pixel_to_rgb(yuyv_data: &[u8], x: u32, y: u32, width: u32) -> (u8, u8, u8) {
    // Fixed-point coefficients (multiplied by 1024) - BT.601 standard
    const V_TO_R: i32 = 1437; // 1.402 * 1024
    const U_TO_G: i32 = 352; // 0.344136 * 1024
    const V_TO_G: i32 = 731; // 0.714136 * 1024
    const U_TO_B: i32 = 1814; // 1.772 * 1024

    // YUYV stores 2 pixels in 4 bytes: [Y0 U Y1 V]
    // Calculate offset for the pixel pair containing this pixel
    let pixel_pair_x = (x / 2) * 2; // Align to even pixel
    let yuyv_offset = ((y * width + pixel_pair_x) * 2) as usize;

    // Bounds check
    if yuyv_offset + 3 >= yuyv_data.len() {
        return (0, 0, 0); // Black for out-of-bounds
    }

    // Extract Y, U, V values
    let y_value = if x % 2 == 0 {
        yuyv_data[yuyv_offset] as i32 // Y0 for even pixels
    } else {
        yuyv_data[yuyv_offset + 2] as i32 // Y1 for odd pixels
    };

    let u = yuyv_data[yuyv_offset + 1] as i32 - 128;
    let v = yuyv_data[yuyv_offset + 3] as i32 - 128;

    // Convert YUYV to RGB using BT.601
    let r = (y_value + ((v * V_TO_R) >> 10)).clamp(0, 255) as u8;
    let g = (y_value - ((u * U_TO_G + v * V_TO_G) >> 10)).clamp(0, 255) as u8;
    let b = (y_value + ((u * U_TO_B) >> 10)).clamp(0, 255) as u8;

    (r, g, b)
}

/// Converts YUYV (YUV 4:2:2) format to RGB using the `yuv` crate
///
/// This uses SIMD-optimized conversions with automatic platform detection:
/// - x86_64: AVX2 or SSE4.1
/// - ARM64: NEON
/// - Fallback: Portable scalar code
///
/// YUYV format stores 2 pixels in 4 bytes: [Y0 U Y1 V]
/// where Y0 and Y1 are luminance values for two adjacent pixels,
/// and U, V are shared chrominance values for both pixels.
///
/// Uses ITU-R BT.601 standard with full range (0-255) color space.
///
/// # Performance
/// For full-frame conversion, this is ~10x faster than pixel-by-pixel conversion
/// due to SIMD optimizations. For sampling a small subset of pixels, use
/// `yuyv_pixel_to_rgb` instead to avoid allocating the full RGB buffer.
pub fn yuyv_to_rgb(yuyv_data: &[u8], width: usize, height: usize) -> Vec<u8> {
    // Create packed image wrapper for YUYV data
    // Stride is in components, YUYV has 2 components per pixel (4 bytes per 2 pixels)
    let packed_image = YuvPackedImage {
        yuy: yuyv_data,
        yuy_stride: (width * 2) as u32,
        width: width as u32,
        height: height as u32,
    };

    // Allocate output RGB buffer
    let rgb_size = width * height * 3;
    let mut rgb_data = vec![0u8; rgb_size];
    let rgb_stride = (width * 3) as u32;

    // Convert YUYV to RGB using BT.601 standard with full range
    yuyv422_to_rgb(
        &packed_image,
        &mut rgb_data,
        rgb_stride,
        YuvRange::Full,
        YuvStandardMatrix::Bt601,
    )
    .expect("YUYV to RGB conversion failed");

    rgb_data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_black_pixels() {
        // Black: Y=0, U=128, V=128 (neutral chroma)
        let yuyv = vec![0, 128, 0, 128];
        let rgb = yuyv_to_rgb(&yuyv, 2, 1);

        // Should produce black (0, 0, 0) for both pixels
        assert_eq!(rgb, vec![0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn test_white_pixels() {
        // White: Y=255, U=128, V=128 (neutral chroma)
        let yuyv = vec![255, 128, 255, 128];
        let rgb = yuyv_to_rgb(&yuyv, 2, 1);

        // Should produce white (255, 255, 255) for both pixels
        assert_eq!(rgb, vec![255, 255, 255, 255, 255, 255]);
    }

    #[test]
    fn test_gray_pixels() {
        // Gray: Y=128, U=128, V=128 (neutral chroma)
        let yuyv = vec![128, 128, 128, 128];
        let rgb = yuyv_to_rgb(&yuyv, 2, 1);

        // Should produce gray (128, 128, 128) for both pixels
        assert_eq!(rgb, vec![128, 128, 128, 128, 128, 128]);
    }

    #[test]
    fn test_output_dimensions() {
        // 4 pixels (2x2) = 8 YUYV bytes = 12 RGB bytes
        let yuyv = vec![128, 128, 128, 128, 128, 128, 128, 128];
        let rgb = yuyv_to_rgb(&yuyv, 4, 1);

        // Should produce 4 pixels * 3 bytes = 12 bytes
        assert_eq!(rgb.len(), 12);
    }

    #[test]
    fn test_red_tinted_pixels() {
        // Red tint: higher V value
        // Y=128, U=128, V=200 should produce reddish gray
        let yuyv = vec![128, 128, 128, 200];
        let rgb = yuyv_to_rgb(&yuyv, 2, 1);

        // Red component should be higher than green/blue
        assert!(rgb[0] > rgb[1]); // R > G
        assert!(rgb[0] > rgb[2]); // R > B
    }

    #[test]
    fn test_blue_tinted_pixels() {
        // Blue tint: higher U value
        // Y=128, U=200, V=128 should produce blueish gray
        let yuyv = vec![128, 200, 128, 128];
        let rgb = yuyv_to_rgb(&yuyv, 2, 1);

        // Blue component should be higher than red/green
        assert!(rgb[2] > rgb[0]); // B > R
        assert!(rgb[2] > rgb[1]); // B > G
    }

    #[test]
    fn test_pixel_conversion_consistency() {
        // Verify that pixel-by-pixel conversion matches full-frame conversion
        let yuyv = vec![
            0, 128, 0, 128, // Black pixels
            255, 128, 255, 128, // White pixels
            128, 128, 128, 128, // Gray pixels
            128, 128, 128, 200, // Red-tinted pixels
        ];

        // Full-frame conversion
        let full_rgb = yuyv_to_rgb(&yuyv, 8, 1);

        // Pixel-by-pixel conversion
        for x in 0..8 {
            let (r, g, b) = yuyv_pixel_to_rgb(&yuyv, x, 0, 8);
            let offset = (x * 3) as usize;

            // Should match within rounding tolerance (±1)
            assert!(
                (full_rgb[offset] as i16 - r as i16).abs() <= 1,
                "Red mismatch at pixel {}: full={}, pixel={}",
                x,
                full_rgb[offset],
                r
            );
            assert!(
                (full_rgb[offset + 1] as i16 - g as i16).abs() <= 1,
                "Green mismatch at pixel {}: full={}, pixel={}",
                x,
                full_rgb[offset + 1],
                g
            );
            assert!(
                (full_rgb[offset + 2] as i16 - b as i16).abs() <= 1,
                "Blue mismatch at pixel {}: full={}, pixel={}",
                x,
                full_rgb[offset + 2],
                b
            );
        }
    }

    #[test]
    fn test_pixel_conversion_bounds() {
        // Test out-of-bounds handling
        let yuyv = vec![128, 128, 128, 128];
        let (r, g, b) = yuyv_pixel_to_rgb(&yuyv, 10, 0, 2);

        // Should return black for out-of-bounds
        assert_eq!((r, g, b), (0, 0, 0));
    }
}
