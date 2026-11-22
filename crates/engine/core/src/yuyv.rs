//! YUYV (packed 4:2:2) sampling without converting the whole frame.
use crate::{Frame, PixelFormat, YuvToRgb};

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
        self.row(y).rgb(x as usize)
    }

    /// Row `y`.
    ///
    /// # Panics
    /// If `y` is outside the frame.
    #[inline]
    #[must_use]
    pub fn row(&self, y: u32) -> YuyvRow<'a> {
        let length = self.width as usize * 2;
        let start = y as usize * length;
        YuyvRow {
            data: &self.data[start..start + length],
            convert: self.convert,
        }
    }
}

/// One row of a YUYV frame: `[Y0 U Y1 V]` pairs. A frame's YUYV width is
/// even, so every pixel's pair is whole. Each accessor panics for a pixel
/// outside the row.
#[derive(Debug, Clone, Copy)]
pub struct YuyvRow<'a> {
    data: &'a [u8],
    convert: YuvToRgb,
}

impl YuyvRow<'_> {
    /// Pixel `x`'s luma.
    #[inline]
    #[must_use]
    pub fn luma(&self, x: usize) -> u8 {
        self.data[x * 2]
    }

    /// Pixel `x`'s chroma `(U, V)`, which it shares with its pair.
    #[inline]
    #[must_use]
    pub fn chroma(&self, x: usize) -> (u8, u8) {
        let pair = (x & !1) * 2;
        (self.data[pair + 1], self.data[pair + 3])
    }

    /// Pixel `x` as 8-bit RGB.
    #[inline]
    #[must_use]
    pub fn rgb(&self, x: usize) -> (u8, u8, u8) {
        let (u, v) = self.chroma(x);
        self.convert.rgb(self.luma(x), u, v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{YuvRange, YuvStandardMatrix};

    #[test]
    fn limited_range_black_and_white() {
        for matrix in [
            YuvStandardMatrix::Bt601,
            YuvStandardMatrix::Bt709,
            YuvStandardMatrix::Bt2020,
        ] {
            for (y, expected) in [(16, 0), (235, 255)] {
                let data = [y, 128, y, 128];
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
}
