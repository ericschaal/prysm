use crate::{Frame, PixelFormat, YuvToRgb};
use yuv::{YuvPackedImage, YuvRange, YuvStandardMatrix, yuyv422_to_rgb};

/// Decode one YUYV pixel using the frame's range and color matrix.
/// Builds the coefficients per call; loops should use [`YuvToRgb`] directly.
#[inline]
#[must_use]
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
    YuvToRgb::with_precision(range, matrix, 6).rgb(pair[(x % 2) as usize * 2], pair[1], pair[3])
}

/// Converts YUYV (YUV 4:2:2) format to RGB using the `yuv` crate
///
/// This uses SIMD-optimized conversions with automatic platform detection:
/// - `x86_64`: AVX2 or SSE4.1
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
///
/// # Errors
/// Returns an error for a non-YUYV frame, RGB size overflow, or a decoder failure.
pub fn yuyv_to_rgb(frame: &Frame) -> Result<Vec<u8>, ConversionError> {
    let mut rgb_data = Vec::new();
    yuyv_to_rgb_into(frame, &mut rgb_data)?;
    Ok(rgb_data)
}

/// A frame cannot be converted to RGB using the YUYV or NV12 decoder.
#[derive(Debug)]
pub enum ConversionError {
    NotYuyv,
    NotNv12,
    SizeOverflow,
    Decode(yuv::YuvError),
}

impl std::fmt::Display for ConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotYuyv => f.write_str("expected a YUYV frame"),
            Self::NotNv12 => f.write_str("expected an NV12 frame"),
            Self::SizeOverflow => f.write_str("RGB dimensions exceed the supported buffer size"),
            Self::Decode(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ConversionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode(error) => Some(error),
            _ => None,
        }
    }
}

/// Like [`yuyv_to_rgb`], but reuses the output allocation. Empty frames clear it.
///
/// # Errors
/// Returns an error for a non-YUYV frame, RGB size overflow, or a decoder failure.
pub fn yuyv_to_rgb_into(frame: &Frame, rgb_out: &mut Vec<u8>) -> Result<(), ConversionError> {
    let PixelFormat::YUYV { range, matrix } = frame.format() else {
        return Err(ConversionError::NotYuyv);
    };
    let (width, height) = (frame.width(), frame.height());
    if width == 0 || height == 0 {
        rgb_out.clear();
        return Ok(());
    }
    let rgb_size = PixelFormat::RGB24
        .expected_size(width, height)
        .map_err(|_| ConversionError::SizeOverflow)?
        .ok_or(ConversionError::SizeOverflow)?;
    let rgb_stride = width.checked_mul(3).ok_or(ConversionError::SizeOverflow)?;
    let packed_image = YuvPackedImage {
        yuy: frame.as_slice(),
        yuy_stride: width.checked_mul(2).ok_or(ConversionError::SizeOverflow)?,
        width,
        height,
    };
    rgb_out.resize(rgb_size, 0);
    yuyv422_to_rgb(&packed_image, rgb_out, rgb_stride, range, matrix)
        .map_err(ConversionError::Decode)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(
        data: &[u8],
        width: u32,
        height: u32,
        range: YuvRange,
        matrix: YuvStandardMatrix,
    ) -> Vec<u8> {
        let frame = Frame::new(
            data.to_vec(),
            width,
            height,
            PixelFormat::YUYV { range, matrix },
        )
        .unwrap();
        yuyv_to_rgb(&frame).unwrap()
    }

    #[test]
    fn conversion_handles_empty_frames_and_rejects_other_formats() {
        let format = PixelFormat::YUYV {
            range: YuvRange::Full,
            matrix: YuvStandardMatrix::Bt601,
        };
        for (width, height) in [(0, 0), (0, 2), (2, 0)] {
            let frame = Frame::new(vec![], width, height, format).unwrap();
            let mut output = vec![1; 6];
            yuyv_to_rgb_into(&frame, &mut output).unwrap();
            assert_eq!(output, [] as [u8; 0]);
        }
        assert!(matches!(
            yuyv_to_rgb(&Frame::dummy(2, 1).unwrap()),
            Err(ConversionError::NotYuyv)
        ));
        assert!(Frame::new(vec![], 2, 1, format).is_err());
    }

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
                    decode(&data, 2, 1, YuvRange::Limited, matrix),
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
                            let full = decode(&data, 2, 1, range, matrix);
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
        let rgb = decode(&yuyv, 2, 1, YuvRange::Full, YuvStandardMatrix::Bt601);

        // Should produce black (0, 0, 0) for both pixels
        assert_eq!(rgb, vec![0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn test_white_pixels() {
        // White: Y=255, U=128, V=128 (neutral chroma)
        let yuyv = vec![255, 128, 255, 128];
        let rgb = decode(&yuyv, 2, 1, YuvRange::Full, YuvStandardMatrix::Bt601);

        // Should produce white (255, 255, 255) for both pixels
        assert_eq!(rgb, vec![255, 255, 255, 255, 255, 255]);
    }

    #[test]
    fn test_gray_pixels() {
        // Gray: Y=128, U=128, V=128 (neutral chroma)
        let yuyv = vec![128, 128, 128, 128];
        let rgb = decode(&yuyv, 2, 1, YuvRange::Full, YuvStandardMatrix::Bt601);

        // Should produce gray (128, 128, 128) for both pixels
        assert_eq!(rgb, vec![128, 128, 128, 128, 128, 128]);
    }

    #[test]
    fn test_output_dimensions() {
        // 4 pixels (2x2) = 8 YUYV bytes = 12 RGB bytes
        let yuyv = vec![128, 128, 128, 128, 128, 128, 128, 128];
        let rgb = decode(&yuyv, 4, 1, YuvRange::Full, YuvStandardMatrix::Bt601);

        // Should produce 4 pixels * 3 bytes = 12 bytes
        assert_eq!(rgb.len(), 12);
    }

    #[test]
    fn test_red_tinted_pixels() {
        // Red tint: higher V value
        // Y=128, U=128, V=200 should produce reddish gray
        let yuyv = vec![128, 128, 128, 200];
        let rgb = decode(&yuyv, 2, 1, YuvRange::Full, YuvStandardMatrix::Bt601);

        // Red component should be higher than green/blue
        assert!(rgb[0] > rgb[1]); // R > G
        assert!(rgb[0] > rgb[2]); // R > B
    }

    #[test]
    fn test_blue_tinted_pixels() {
        // Blue tint: higher U value
        // Y=128, U=200, V=128 should produce blueish gray
        let yuyv = vec![128, 200, 128, 128];
        let rgb = decode(&yuyv, 2, 1, YuvRange::Full, YuvStandardMatrix::Bt601);

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
        let full_rgb = decode(&yuyv, 8, 1, YuvRange::Full, YuvStandardMatrix::Bt601);

        // Pixel-by-pixel conversion
        for x in 0..8 {
            let (r, g, b) =
                yuyv_pixel_to_rgb(&yuyv, x, 0, 8, YuvRange::Full, YuvStandardMatrix::Bt601);
            let offset = (x * 3) as usize;

            // Should match within rounding tolerance (±1)
            assert!(
                (i16::from(full_rgb[offset]) - i16::from(r)).abs() <= 1,
                "Red mismatch at pixel {}: full={}, pixel={}",
                x,
                full_rgb[offset],
                r
            );
            assert!(
                (i16::from(full_rgb[offset + 1]) - i16::from(g)).abs() <= 1,
                "Green mismatch at pixel {}: full={}, pixel={}",
                x,
                full_rgb[offset + 1],
                g
            );
            assert!(
                (i16::from(full_rgb[offset + 2]) - i16::from(b)).abs() <= 1,
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
