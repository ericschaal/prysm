//! YUYV (packed 4:2:2) sampling and whole-frame conversion.
use crate::{Frame, PixelFormat, YuvToRgb};
use yuv::{YuvPackedImage, yuyv422_to_rgb};

/// A borrowed YUYV frame plus its precomputed colour conversion. Reads only
/// the pixels asked for, at full precision (the `yuv` crate's YUYV path uses
/// 6-bit coefficients).
#[derive(Debug, Clone, Copy)]
pub struct YuyvView<'a> {
    data: &'a [u8],
    width: u32,
    height: u32,
    convert: YuvToRgb,
}

impl<'a> YuyvView<'a> {
    /// `None` unless `frame` is YUYV.
    #[must_use]
    pub fn new(frame: &'a Frame) -> Option<Self> {
        let PixelFormat::YUYV { range, matrix } = frame.format() else {
            return None;
        };
        Some(Self {
            data: frame.as_slice(),
            width: frame.width(),
            height: frame.height(),
            convert: YuvToRgb::new(range, matrix),
        })
    }

    /// One pixel; black outside the frame.
    #[inline]
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> (u8, u8, u8) {
        if x >= self.width || y >= self.height {
            return (0, 0, 0);
        }
        // A frame's YUYV width is even, so the pixel's pair `[Y0 U Y1 V]` is whole.
        let pair = (y as usize * self.width as usize + (x as usize & !1)) * 2;
        let luma = self.data[pair + (x as usize & 1) * 2];
        self.convert
            .rgb(luma, self.data[pair + 1], self.data[pair + 3])
    }
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
/// Its 6-bit coefficients put a channel up to 3 codes off the exact maths;
/// sampling should use [`YuyvView`], which reads only the pixels it needs.
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
    use crate::{YuvRange, YuvStandardMatrix};

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
                let frame = Frame::new(
                    data.to_vec(),
                    2,
                    1,
                    PixelFormat::YUYV {
                        range: YuvRange::Limited,
                        matrix,
                    },
                )
                .unwrap();
                assert_eq!(
                    YuyvView::new(&frame).unwrap().pixel(0, 0),
                    (expected, expected, expected)
                );
            }
        }
    }

    #[test]
    fn view_reads_each_pixel_with_its_pairs_chroma_and_black_outside() {
        let (range, matrix) = (YuvRange::Limited, YuvStandardMatrix::Bt709);
        // 4x2, pairs [Y0 U Y1 V].
        let data = [
            20, 60, 40, 200, 80, 100, 120, 140, // row 0
            160, 180, 200, 30, 220, 50, 235, 70, // row 1
        ];
        let frame = Frame::new(data.to_vec(), 4, 2, PixelFormat::YUYV { range, matrix }).unwrap();
        let view = YuyvView::new(&frame).unwrap();
        let convert = YuvToRgb::new(range, matrix);
        for y in 0..2 {
            for x in 0..4 {
                let pair = (y * 4 + x / 2 * 2) * 2;
                let luma = data[pair + x % 2 * 2];
                assert_eq!(
                    view.pixel(u32::try_from(x).unwrap(), u32::try_from(y).unwrap()),
                    convert.rgb(luma, data[pair + 1], data[pair + 3]),
                    "({x}, {y})"
                );
            }
        }
        assert_eq!(view.pixel(4, 0), (0, 0, 0));
        assert_eq!(view.pixel(0, 2), (0, 0, 0));
        assert!(YuyvView::new(&Frame::dummy(2, 1).unwrap()).is_none());
    }

    /// The whole-frame decode is the `yuv` crate's 6-bit fast path, so it
    /// lands within a few codes of the view's (exact) result, not one.
    #[test]
    fn full_decode_stays_within_its_precision_of_the_view() {
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
                            let frame = Frame::new(
                                data.to_vec(),
                                2,
                                1,
                                PixelFormat::YUYV { range, matrix },
                            )
                            .unwrap();
                            let (r, g, b) = YuyvView::new(&frame).unwrap().pixel(1, 0);
                            for (actual, expected) in [r, g, b].into_iter().zip(full) {
                                assert!(
                                    actual.abs_diff(expected) <= 3,
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
}
