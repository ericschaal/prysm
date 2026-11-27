use prysm_core::nv12::Nv12View;
use prysm_core::yuyv::YuyvView;
use prysm_core::{Color, Frame, PixelFormat};

/// A frame's pixels, read in place in its own format: built once per frame
/// for black-bar detection, cell averaging and previews, so that each pixel layout is
/// decoded in one place (`prysm_core`'s YUYV and NV12 rows, the RGB rows here).
///
/// Frames are validated, so every row of the frame is in the buffer.
/// Coordinates outside the frame are a bug of the caller: every accessor
/// panics for them.
#[derive(Debug, Clone, Copy)]
pub struct PixelView<'a>(Layout<'a>);

#[derive(Debug, Clone, Copy)]
enum Layout<'a> {
    Rgb(&'a Frame),
    Bgr(&'a Frame),
    Yuyv(YuyvView<'a>),
    Nv12(Nv12View<'a>),
}

/// The packed three-byte pixels in row `y` of an RGB24 or BGR24 frame.
#[inline]
fn rgb_row(frame: &Frame, y: u32) -> &[[u8; 3]] {
    let width = frame.width() as usize;
    let start = y as usize * width;
    &frame.as_slice().as_chunks::<3>().0[start..start + width]
}

impl<'a> PixelView<'a> {
    pub fn new(frame: &'a Frame) -> Self {
        let view = "a frame of the format has its view";
        Self(match frame.format() {
            PixelFormat::RGB24 => Layout::Rgb(frame),
            PixelFormat::BGR24 => Layout::Bgr(frame),
            PixelFormat::YUYV { .. } => Layout::Yuyv(YuyvView::new(frame).expect(view)),
            PixelFormat::NV12 { .. } => Layout::Nv12(Nv12View::new(frame).expect(view)),
        })
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
        match self.0 {
            Layout::Rgb(frame) | Layout::Bgr(frame) => {
                let [r, g, b] = rgb_row(frame, y)[x];
                (r.max(g).max(b), 0)
            }
            Layout::Yuyv(view) => {
                let row = view.row(y);
                (row.luma(x), distance(row.chroma(x)))
            }
            Layout::Nv12(view) => {
                let row = view.row(y);
                (row.luma(x), distance(row.chroma(x)))
            }
        }
    }

    /// Calls `each` with the 8-bit RGB colour of the selected pixels
    /// in row `y`, in the order given by `xs`.
    #[inline]
    pub fn for_each_rgb(
        &self,
        y: u32,
        xs: impl Iterator<Item = usize>,
        mut each: impl FnMut(Color),
    ) {
        match self.0 {
            Layout::Rgb(frame) => {
                let row = rgb_row(frame, y);
                for x in xs {
                    let [r, g, b] = row[x];
                    each(Color::new(r, g, b));
                }
            }
            Layout::Bgr(frame) => {
                let row = rgb_row(frame, y);
                for x in xs {
                    let [b, g, r] = row[x];
                    each(Color::new(r, g, b));
                }
            }
            Layout::Yuyv(view) => {
                let row = view.row(y);
                for x in xs {
                    let (r, g, b) = row.rgb(x);
                    each(Color::new(r, g, b));
                }
            }
            Layout::Nv12(view) => {
                let row = view.row(y);
                for x in xs {
                    let (r, g, b) = row.rgb(x);
                    each(Color::new(r, g, b));
                }
            }
        }
    }
}
