//! NV12 (planar 4:2:0) sampling without converting the whole frame.
use crate::{Frame, PixelFormat, YuvToRgb};

/// Borrowed NV12 planes plus the frame's precomputed color conversion.
/// Reads only the pixels asked for.
#[derive(Debug, Clone, Copy)]
pub struct Nv12View<'a> {
    y: &'a [u8],
    uv: &'a [u8],
    width: u32,
    height: u32,
    convert: YuvToRgb,
}

impl<'a> Nv12View<'a> {
    /// `None` unless `frame` is NV12.
    #[must_use]
    pub fn new(frame: &'a Frame) -> Option<Self> {
        let PixelFormat::NV12 { range, matrix } = frame.format() else {
            return None;
        };
        let (y, uv) = frame.nv12_planes()?;
        Some(Self {
            y,
            uv,
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
        self.row(y).rgb(x as usize)
    }

    /// Row `y`: its luma, and the chroma row it shares with its pair of rows.
    ///
    /// # Panics
    /// If `y` is outside the frame.
    #[inline]
    #[must_use]
    pub fn row(&self, y: u32) -> Nv12Row<'a> {
        let (width, y) = (self.width as usize, y as usize);
        Nv12Row {
            luma: &self.y[y * width..(y + 1) * width],
            chroma: &self.uv[y / 2 * width..(y / 2 + 1) * width],
            convert: self.convert,
        }
    }
}

/// One row of an NV12 frame: `width` luma bytes, and `width / 2`
/// interleaved `U V` pairs, one per pair of pixels. Each accessor panics for
/// a pixel outside the row.
#[derive(Debug, Clone, Copy)]
pub struct Nv12Row<'a> {
    luma: &'a [u8],
    chroma: &'a [u8],
    convert: YuvToRgb,
}

impl Nv12Row<'_> {
    /// Pixel `x`'s luma.
    #[inline]
    #[must_use]
    pub fn luma(&self, x: usize) -> u8 {
        self.luma[x]
    }

    /// Pixel `x`'s chroma `(U, V)`, which it shares with its 2x2 block.
    #[inline]
    #[must_use]
    pub fn chroma(&self, x: usize) -> (u8, u8) {
        let pair = x & !1;
        (self.chroma[pair], self.chroma[pair + 1])
    }

    /// Pixel `x` as 8-bit RGB.
    #[inline]
    #[must_use]
    pub fn rgb(&self, x: usize) -> (u8, u8, u8) {
        let (u, v) = self.chroma(x);
        self.convert.rgb(self.luma(x), u, v)
    }
}

/// A frame cannot be converted to RGB by [`nv12_to_rgb`].
#[cfg(any(test, feature = "testkit"))]
#[derive(Debug, thiserror::Error)]
pub enum ConversionError {
    #[error("expected an NV12 frame")]
    NotNv12,
    #[error("RGB dimensions exceed the supported buffer size")]
    SizeOverflow,
    #[error("{0}")]
    Decode(#[source] yuv::YuvError),
}

/// Converts a whole NV12 frame to RGB24 with the `yuv` crate's SIMD path: an
/// independent reference for tests of [`Nv12View`] and its users, with the
/// `testkit` feature.
///
/// # Errors
/// Returns an error for a non-NV12 frame, RGB size overflow, or a decoder failure.
/// Empty frames give an empty buffer.
#[cfg(any(test, feature = "testkit"))]
pub fn nv12_to_rgb(frame: &Frame) -> Result<Vec<u8>, ConversionError> {
    let PixelFormat::NV12 { range, matrix } = frame.format() else {
        return Err(ConversionError::NotNv12);
    };
    let (width, height) = (frame.width(), frame.height());
    if width == 0 || height == 0 {
        return Ok(Vec::new());
    }
    let (y_plane, uv_plane) = frame.nv12_planes().ok_or(ConversionError::NotNv12)?;
    let rgb_size = PixelFormat::RGB24
        .expected_size(width, height)
        .map_err(|_| ConversionError::SizeOverflow)?;
    let mut rgb = vec![0; rgb_size];
    yuv::yuv_nv12_to_rgb(
        &yuv::YuvBiPlanarImage {
            y_plane,
            y_stride: width,
            uv_plane,
            uv_stride: width,
            width,
            height,
        },
        &mut rgb,
        width.checked_mul(3).ok_or(ConversionError::SizeOverflow)?,
        range,
        matrix,
        yuv::YuvConversionMode::Balanced,
    )
    .map_err(ConversionError::Decode)?;
    Ok(rgb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{YuvRange, YuvStandardMatrix};

    fn frame(width: u32, height: u32, range: YuvRange, matrix: YuvStandardMatrix) -> Frame {
        let size = (width * height * 3 / 2) as usize;
        // Legal limited-range codes (16..=235), where the `yuv` crate's
        // NV12 path and ours agree; it clamps sub-black luma, YUYV does not.
        let data = (0..size)
            .map(|i| u8::try_from(16 + (i * 37 + i / 7 * 11) % 220).unwrap())
            .collect();
        Frame::new(data, width, height, PixelFormat::NV12 { range, matrix }).unwrap()
    }

    #[test]
    fn sampling_matches_the_full_conversion_for_each_range_and_matrix() {
        for range in [YuvRange::Full, YuvRange::Limited] {
            for matrix in [
                YuvStandardMatrix::Bt601,
                YuvStandardMatrix::Bt709,
                YuvStandardMatrix::Bt2020,
            ] {
                let frame = frame(18, 10, range, matrix);
                let view = Nv12View::new(&frame).unwrap();
                let full = nv12_to_rgb(&frame).unwrap();
                for y in 0..10 {
                    for x in 0..18 {
                        let (r, g, b) = view.pixel(x, y);
                        let at = ((y * 18 + x) * 3) as usize;
                        for (actual, expected) in [r, g, b].into_iter().zip(&full[at..at + 3]) {
                            assert!(
                                actual.abs_diff(*expected) <= 1,
                                "{range:?} {matrix:?} ({x}, {y}): {actual} != {expected}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn limited_range_black_and_white_and_neighbors_share_chroma() {
        let format = PixelFormat::NV12 {
            range: YuvRange::Limited,
            matrix: YuvStandardMatrix::Bt709,
        };
        // Y: 16 235 / 235 16; UV: neutral.
        let frame = Frame::new(vec![16, 235, 235, 16, 128, 128], 2, 2, format).unwrap();
        let view = Nv12View::new(&frame).unwrap();
        assert_eq!(view.pixel(0, 0), (0, 0, 0));
        assert_eq!(view.pixel(1, 0), (255, 255, 255));
        assert_eq!(view.pixel(0, 1), (255, 255, 255));
        assert_eq!(view.pixel(2, 0), (0, 0, 0));
        // Red chroma tints both pixels of a pair, on both rows of the pair.
        let red = Frame::new(
            vec![128; 4].into_iter().chain([128, 220]).collect(),
            2,
            2,
            format,
        )
        .unwrap();
        let view = Nv12View::new(&red).unwrap();
        for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let (r, g, b) = view.pixel(x, y);
            assert!(r > g && r > b, "({x}, {y})");
        }
    }

    #[test]
    fn decoder_errors_retain_their_message_and_cause() {
        let error = ConversionError::Decode(yuv::YuvError::PointerOverflow);
        let cause = std::error::Error::source(&error).unwrap();
        assert!(cause.is::<yuv::YuvError>());
        assert_eq!(error.to_string(), cause.to_string());
    }

    #[test]
    fn other_formats_are_rejected_and_empty_frames_clear() {
        let rgb = Frame::dummy(2, 2).unwrap();
        assert!(Nv12View::new(&rgb).is_none());
        assert!(matches!(nv12_to_rgb(&rgb), Err(ConversionError::NotNv12)));
        let format = PixelFormat::NV12 {
            range: YuvRange::Full,
            matrix: YuvStandardMatrix::Bt601,
        };
        let empty = Frame::new(vec![], 0, 2, format).unwrap();
        assert_eq!(nv12_to_rgb(&empty).unwrap(), [] as [u8; 0]);
    }
}
