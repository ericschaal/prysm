use yuv::{YuvPackedImage, YuvRange, YuvStandardMatrix, yuyv422_to_rgb};

/// Decode one YUYV pixel using the frame's range and color matrix.
#[inline]
pub fn yuyv_pixel_to_rgb(
    data: &[u8],
    x: u32,
    y: u32,
    width: u32,
    range: YuvRange,
    matrix: YuvStandardMatrix,
) -> (u8, u8, u8) {
    if x >= width || !width.is_multiple_of(2) {
        return (0, 0, 0);
    }
    let offset = (y as usize * width as usize + (x as usize / 2) * 2) * 2;
    let Some(pair) = data.get(offset..offset + 4) else {
        return (0, 0, 0);
    };
    let (bias, y_scale, uv_scale) = match range {
        YuvRange::Full => (0.0, 1.0, 1.0),
        YuvRange::Limited => (16.0, 255.0 / 219.0, 255.0 / 224.0),
    };
    let coefficients = matrix.get_kr_kb();
    let (kr, kb) = (coefficients.kr, coefficients.kb);
    let kg = 1.0 - kr - kb;
    // Match the yuv crate's 6-bit fixed-point coefficients and rounding.
    let y_coef = (y_scale * 64.0_f32).round() as i32;
    let cr_coef = (2.0 * (1.0 - kr) * uv_scale * 64.0).round() as i32;
    let cb_coef = (2.0 * (1.0 - kb) * uv_scale * 64.0).round() as i32;
    let g_cr = (2.0 * (1.0 - kr) * kr / kg * uv_scale * 64.0).round() as i32;
    let g_cb = (2.0 * (1.0 - kb) * kb / kg * uv_scale * 64.0).round() as i32;
    let y = (i32::from(pair[(x % 2) as usize * 2]) - bias as i32) * y_coef;
    let u = i32::from(pair[1]) - 128;
    let v = i32::from(pair[3]) - 128;
    let r = ((y + cr_coef * v + 32) >> 6).clamp(0, 255) as u8;
    let g = ((y - g_cr * v - g_cb * u + 32) >> 6).clamp(0, 255) as u8;
    let b = ((y + cb_coef * u + 32) >> 6).clamp(0, 255) as u8;
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
/// Uses the supplied frame range and color matrix.
///
/// # Performance
/// For full-frame conversion, this is ~10x faster than pixel-by-pixel conversion
/// due to SIMD optimizations. For sampling a small subset of pixels, use
/// `yuyv_pixel_to_rgb` instead to avoid allocating the full RGB buffer.
pub fn yuyv_to_rgb(
    yuyv_data: &[u8],
    width: usize,
    height: usize,
    range: YuvRange,
    matrix: YuvStandardMatrix,
) -> Vec<u8> {
    let mut rgb_data = Vec::new();
    yuyv_to_rgb_into(yuyv_data, &mut rgb_data, width, height, range, matrix);
    rgb_data
}

/// Like [`yuyv_to_rgb`], but writes into a caller-provided buffer so that hot
/// paths can reuse the allocation across frames.
pub fn yuyv_to_rgb_into(
    yuyv_data: &[u8],
    rgb_out: &mut Vec<u8>,
    width: usize,
    height: usize,
    range: YuvRange,
    matrix: YuvStandardMatrix,
) {
    // Create packed image wrapper for YUYV data
    // Stride is in components, YUYV has 2 components per pixel (4 bytes per 2 pixels)
    let packed_image = YuvPackedImage {
        yuy: yuyv_data,
        yuy_stride: (width * 2) as u32,
        width: width as u32,
        height: height as u32,
    };

    rgb_out.resize(width * height * 3, 0);
    let rgb_stride = (width * 3) as u32;

    yuyv422_to_rgb(&packed_image, rgb_out, rgb_stride, range, matrix)
        .expect("YUYV to RGB conversion failed");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limited_range_black_and_white() {
        for matrix in [
            YuvStandardMatrix::Bt601,
            YuvStandardMatrix::Bt709,
            YuvStandardMatrix::Bt2020,
        ] {
            for (y, expected) in [(16, 0), (235, 255)] {
                let data = [y, 128, y, 128];
                assert_eq!(
                    yuyv_to_rgb(&data, 2, 1, YuvRange::Limited, matrix),
                    [expected; 6]
                );
                assert_eq!(
                    yuyv_pixel_to_rgb(&data, 0, 0, 2, YuvRange::Limited, matrix),
                    (expected, expected, expected)
                );
            }
        }
    }

    #[test]
    fn scalar_and_full_decode_agree_for_each_matrix_and_range() {
        for range in [YuvRange::Full, YuvRange::Limited] {
            for matrix in [
                YuvStandardMatrix::Bt601,
                YuvStandardMatrix::Bt709,
                YuvStandardMatrix::Bt2020,
                YuvStandardMatrix::Smpte240,
            ] {
                for y in [0, 16, 64, 128, 235, 255] {
                    for u in [0, 64, 128, 192, 255] {
                        for v in [0, 64, 128, 192, 255] {
                            let data = [y, u, y, v];
                            let full = yuyv_to_rgb(&data, 2, 1, range, matrix);
                            let (r, g, b) = yuyv_pixel_to_rgb(&data, 1, 0, 2, range, matrix);
                            for (actual, expected) in [r, g, b].into_iter().zip(full) {
                                assert!(
                                    actual.abs_diff(expected) <= 1,
                                    "{range:?} {matrix:?}, {data:?}: {actual} != {expected}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn test_black_pixels() {
        // Black: Y=0, U=128, V=128 (neutral chroma)
        let yuyv = vec![0, 128, 0, 128];
        let rgb = yuyv_to_rgb(&yuyv, 2, 1, YuvRange::Full, YuvStandardMatrix::Bt601);

        // Should produce black (0, 0, 0) for both pixels
        assert_eq!(rgb, vec![0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn test_white_pixels() {
        // White: Y=255, U=128, V=128 (neutral chroma)
        let yuyv = vec![255, 128, 255, 128];
        let rgb = yuyv_to_rgb(&yuyv, 2, 1, YuvRange::Full, YuvStandardMatrix::Bt601);

        // Should produce white (255, 255, 255) for both pixels
        assert_eq!(rgb, vec![255, 255, 255, 255, 255, 255]);
    }

    #[test]
    fn test_gray_pixels() {
        // Gray: Y=128, U=128, V=128 (neutral chroma)
        let yuyv = vec![128, 128, 128, 128];
        let rgb = yuyv_to_rgb(&yuyv, 2, 1, YuvRange::Full, YuvStandardMatrix::Bt601);

        // Should produce gray (128, 128, 128) for both pixels
        assert_eq!(rgb, vec![128, 128, 128, 128, 128, 128]);
    }

    #[test]
    fn test_output_dimensions() {
        // 4 pixels (2x2) = 8 YUYV bytes = 12 RGB bytes
        let yuyv = vec![128, 128, 128, 128, 128, 128, 128, 128];
        let rgb = yuyv_to_rgb(&yuyv, 4, 1, YuvRange::Full, YuvStandardMatrix::Bt601);

        // Should produce 4 pixels * 3 bytes = 12 bytes
        assert_eq!(rgb.len(), 12);
    }

    #[test]
    fn test_red_tinted_pixels() {
        // Red tint: higher V value
        // Y=128, U=128, V=200 should produce reddish gray
        let yuyv = vec![128, 128, 128, 200];
        let rgb = yuyv_to_rgb(&yuyv, 2, 1, YuvRange::Full, YuvStandardMatrix::Bt601);

        // Red component should be higher than green/blue
        assert!(rgb[0] > rgb[1]); // R > G
        assert!(rgb[0] > rgb[2]); // R > B
    }

    #[test]
    fn test_blue_tinted_pixels() {
        // Blue tint: higher U value
        // Y=128, U=200, V=128 should produce blueish gray
        let yuyv = vec![128, 200, 128, 128];
        let rgb = yuyv_to_rgb(&yuyv, 2, 1, YuvRange::Full, YuvStandardMatrix::Bt601);

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
        let full_rgb = yuyv_to_rgb(&yuyv, 8, 1, YuvRange::Full, YuvStandardMatrix::Bt601);

        // Pixel-by-pixel conversion
        for x in 0..8 {
            let (r, g, b) =
                yuyv_pixel_to_rgb(&yuyv, x, 0, 8, YuvRange::Full, YuvStandardMatrix::Bt601);
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
        let (r, g, b) =
            yuyv_pixel_to_rgb(&yuyv, 10, 0, 2, YuvRange::Full, YuvStandardMatrix::Bt601);

        // Should return black for out-of-bounds
        assert_eq!((r, g, b), (0, 0, 0));
    }
}
