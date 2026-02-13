use yuv::{yuyv422_to_rgb, YuvPackedImage, YuvRange, YuvStandardMatrix};

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
}
