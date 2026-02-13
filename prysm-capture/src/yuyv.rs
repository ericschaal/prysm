/// Converts YUYV (YUV 4:2:2) format to RGB using ITU-R BT.601 standard
///
/// YUYV format stores 2 pixels in 4 bytes: [Y0 U Y1 V]
/// where Y0 and Y1 are luminance values for two adjacent pixels,
/// and U, V are shared chrominance values for both pixels.
///
/// Uses fixed-point integer arithmetic for performance:
/// - Coefficients multiplied by 1024
/// - Results shifted right by 10 bits
pub fn yuyv_to_rgb(yuyv_data: &[u8], width: usize, height: usize) -> Vec<u8> {
    // Fixed-point coefficients (multiplied by 1024)
    const V_TO_R: i32 = 1437;  // 1.402 * 1024
    const U_TO_G: i32 = 352;   // 0.344136 * 1024
    const V_TO_G: i32 = 731;   // 0.714136 * 1024
    const U_TO_B: i32 = 1814;  // 1.772 * 1024

    let rgb_size = width * height * 3;
    let mut rgb_data = Vec::with_capacity(rgb_size);

    // Process YUYV in chunks of 4 bytes (2 pixels)
    for chunk in yuyv_data.chunks_exact(4) {
        let y0 = chunk[0] as i32;
        let u = chunk[1] as i32 - 128;
        let y1 = chunk[2] as i32;
        let v = chunk[3] as i32 - 128;

        // First pixel (Y0, U, V)
        let r0 = (y0 + ((v * V_TO_R) >> 10)).clamp(0, 255) as u8;
        let g0 = (y0 - ((u * U_TO_G + v * V_TO_G) >> 10)).clamp(0, 255) as u8;
        let b0 = (y0 + ((u * U_TO_B) >> 10)).clamp(0, 255) as u8;

        rgb_data.push(r0);
        rgb_data.push(g0);
        rgb_data.push(b0);

        // Second pixel (Y1, U, V)
        let r1 = (y1 + ((v * V_TO_R) >> 10)).clamp(0, 255) as u8;
        let g1 = (y1 - ((u * U_TO_G + v * V_TO_G) >> 10)).clamp(0, 255) as u8;
        let b1 = (y1 + ((u * U_TO_B) >> 10)).clamp(0, 255) as u8;

        rgb_data.push(r1);
        rgb_data.push(g1);
        rgb_data.push(b1);
    }

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
