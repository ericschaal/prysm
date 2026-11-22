use prysm_core::nv12::Nv12View;
use prysm_core::yuyv::YuyvView;
use prysm_core::{Color, Frame, PixelFormat};
use std::ops::Range;

/// A frame's pixels, read in place in its own format: built once per frame
/// for black-bar detection and cell averaging, so that each pixel layout is
/// decoded in one place (`prysm_core`'s YUYV and NV12 rows, the RGB rows here).
///
/// Frames are validated, so every row of the frame is in the buffer.
/// Coordinates outside the frame are a bug of the caller: every accessor
/// panics for them.
#[derive(Debug, Clone, Copy)]
pub enum PixelView<'a> {
    Rgb(RgbRows<'a>),
    Bgr(RgbRows<'a>),
    Yuyv(YuyvView<'a>),
    Nv12(Nv12View<'a>),
}

/// Rows of three-byte pixels, `width` to a row.
#[derive(Debug, Clone, Copy)]
pub struct RgbRows<'a> {
    pixels: &'a [[u8; 3]],
    width: usize,
}

impl<'a> RgbRows<'a> {
    fn new(frame: &'a Frame) -> Self {
        Self {
            pixels: frame.as_slice().as_chunks::<3>().0,
            width: frame.width() as usize,
        }
    }

    #[inline]
    fn row(&self, y: u32) -> &'a [[u8; 3]] {
        let start = y as usize * self.width;
        &self.pixels[start..start + self.width]
    }
}

impl<'a> PixelView<'a> {
    pub fn new(frame: &'a Frame) -> Self {
        let view = "a frame of the format has its view";
        match frame.format() {
            PixelFormat::RGB24 => Self::Rgb(RgbRows::new(frame)),
            PixelFormat::BGR24 => Self::Bgr(RgbRows::new(frame)),
            PixelFormat::YUYV { .. } => Self::Yuyv(YuyvView::new(frame).expect(view)),
            PixelFormat::NV12 { .. } => Self::Nv12(Nv12View::new(frame).expect(view)),
        }
    }

    /// Pixel (`x`, `y`) as black-bar detection reads it: its brightness, and
    /// how far its chroma is from neutral.
    ///
    /// YUV brightness is the luma, read without a colour conversion; RGB uses
    /// its brightest channel, so that dark coloured pixels are not black, and
    /// counts as neutral.
    #[inline]
    pub fn luma_and_chroma(&self, x: u32, y: u32) -> (u8, u8) {
        let (x, distance) = (x as usize, |(u, v): (u8, u8)| {
            u.abs_diff(128).max(v.abs_diff(128))
        });
        match self {
            Self::Rgb(rows) | Self::Bgr(rows) => {
                let [r, g, b] = rows.row(y)[x];
                (r.max(g).max(b), 0)
            }
            Self::Yuyv(view) => {
                let row = view.row(y);
                (row.luma(x), distance(row.chroma(x)))
            }
            Self::Nv12(view) => {
                let row = view.row(y);
                (row.luma(x), distance(row.chroma(x)))
            }
        }
    }

    /// Calls `each` with the 8-bit RGB colour of every `step`th pixel of `xs`
    /// in row `y`, from the first, in order.
    #[inline]
    pub fn for_each_rgb(&self, y: u32, xs: Range<usize>, step: usize, mut each: impl FnMut(Color)) {
        match self {
            Self::Rgb(rows) => {
                for &[r, g, b] in rows.row(y)[xs].iter().step_by(step) {
                    each(Color::new(r, g, b));
                }
            }
            Self::Bgr(rows) => {
                for &[b, g, r] in rows.row(y)[xs].iter().step_by(step) {
                    each(Color::new(r, g, b));
                }
            }
            Self::Yuyv(view) => {
                let row = view.row(y);
                for x in xs.step_by(step) {
                    let (r, g, b) = row.rgb(x);
                    each(Color::new(r, g, b));
                }
            }
            Self::Nv12(view) => {
                let row = view.row(y);
                for x in xs.step_by(step) {
                    let (r, g, b) = row.rgb(x);
                    each(Color::new(r, g, b));
                }
            }
        }
    }
}
