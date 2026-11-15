use super::linear_fixed::{FRACTION_BITS, add_linear};
use prysm_core::{Frame, LinearColor, PixelFormat, YuvToRgb};
use std::ops::Range;

pub use prysm_core::Viewport;

/// Brightness for black-bar detection, read directly from raw frame data.
///
/// YUYV stores luma at every even byte and NV12 in its first plane, so
/// black-bar detection never needs a color conversion. Out-of-bounds coordinates read as black.
/// RGB uses the brightest channel so dark colored pixels are not treated as black.
pub fn luma_at(frame: &Frame, x: u32, y: u32) -> u8 {
    if x >= frame.width() || y >= frame.height() {
        return 0;
    }
    match frame.format() {
        PixelFormat::YUYV { .. } => {
            let idx = ((y * frame.width() + x) * 2) as usize;
            frame.as_slice().get(idx).copied().unwrap_or(0)
        }
        PixelFormat::NV12 { .. } => {
            let idx = y as usize * frame.width() as usize + x as usize;
            frame.as_slice().get(idx).copied().unwrap_or(0)
        }
        PixelFormat::RGB24 | PixelFormat::BGR24 => {
            let idx = ((y * frame.width() + x) * 3) as usize;
            match frame.as_slice().get(idx..idx + 3) {
                Some(px) => px[0].max(px[1]).max(px[2]),
                None => 0,
            }
        }
        PixelFormat::MJPEG => 0,
    }
}

/// A raw frame plus the viewport that downstream nodes should read from.
///
/// Detection and sampling read the raw pixels; nothing converts a whole frame.
#[derive(Debug, Clone)]
pub struct FrameView {
    pub frame: Frame,
    pub viewport: Viewport,
}

impl FrameView {
    pub fn new(frame: Frame) -> Self {
        let viewport = Viewport::full_frame(frame.width(), frame.height());
        Self { frame, viewport }
    }

    pub fn viewport_width(&self) -> u32 {
        self.viewport.width
    }

    pub fn viewport_height(&self) -> u32 {
        self.viewport.height
    }

    /// Area-average a viewport-relative region in linear RGB, including black pixels.
    #[cfg(test)]
    pub fn average_linear(
        &self,
        x_start: u32,
        y_start: u32,
        x_end: u32,
        y_end: u32,
    ) -> LinearColor {
        let mut color = LinearColor::black();
        let cell = std::slice::from_mut(&mut color);
        self.average_cells(y_start..y_end, &[x_start, x_end], 1, cell);
        color
    }

    /// Area-averages a row of cells in linear RGB, black pixels included.
    /// Cell `i` covers the viewport's `rows` and its columns `x_edges[i]..x_edges[i + 1]`,
    /// clipped to the frame. Only every `step`th pixel of every `step`th row is read,
    /// starting at each cell's first pixel, so a cell always contributes at least
    /// one pixel; `step` 1 reads every pixel. A cell without pixels is black.
    ///
    /// Pixels are summed as exact integers ([`add_linear`]): no floating point
    /// per pixel, the same result in any order, and one division per cell.
    ///
    /// # Panics
    /// Unless there is one more edge than cells.
    pub fn average_cells(
        &self,
        rows: Range<u32>,
        x_edges: &[u32],
        step: u32,
        cells: &mut [LinearColor],
    ) {
        assert_eq!(x_edges.len(), cells.len() + 1, "one more edge than cells");
        cells.fill(LinearColor::black());
        let Some(pixels) = Pixels::new(&self.frame) else {
            return;
        };
        let (width, height) = (self.frame.width(), self.frame.height());
        let clip = |origin: u32, offset: u32, limit: u32| origin.saturating_add(offset).min(limit);
        let rows =
            clip(self.viewport.y, rows.start, height)..clip(self.viewport.y, rows.end, height);
        let spans: Vec<_> = x_edges
            .windows(2)
            .map(|edge| {
                clip(self.viewport.x, edge[0], width) as usize
                    ..clip(self.viewport.x, edge[1], width) as usize
            })
            .collect();
        let step = step.max(1) as usize;
        let mut sums = vec![[0_u64; 3]; cells.len()];
        for y in (rows.start as usize..rows.end as usize).step_by(step) {
            let Some(row) = pixels.row(y, width as usize) else {
                return;
            };
            for (sum, span) in sums.iter_mut().zip(&spans) {
                row.add_span(sum, span.clone(), step);
            }
        }
        let read_rows = rows.len().div_ceil(step);
        for ((cell, sum), span) in cells.iter_mut().zip(sums).zip(&spans) {
            *cell = mean(sum, read_rows * span.len().div_ceil(step));
        }
    }
}

/// A frame's pixels as cell averaging reads them, whole or one row, with
/// their colour conversion.
#[derive(Clone, Copy)]
enum Pixels<'a> {
    Rgb {
        pixels: &'a [[u8; 3]],
        bgr: bool,
    },
    /// `[Y0 U Y1 V]` pairs; a YUYV frame's width is even.
    Yuyv {
        pairs: &'a [[u8; 4]],
        convert: YuvToRgb,
    },
    /// One `[U V]` pair per two pixels and two rows.
    Nv12 {
        luma: &'a [u8],
        chroma: &'a [[u8; 2]],
        convert: YuvToRgb,
    },
}

impl<'a> Pixels<'a> {
    /// `None` for MJPEG, which holds no pixels to read.
    fn new(frame: &'a Frame) -> Option<Self> {
        let data = frame.as_slice();
        Some(match frame.format() {
            PixelFormat::RGB24 | PixelFormat::BGR24 => Self::Rgb {
                pixels: data.as_chunks().0,
                bgr: frame.format() == PixelFormat::BGR24,
            },
            PixelFormat::YUYV { range, matrix } => Self::Yuyv {
                pairs: data.as_chunks().0,
                convert: YuvToRgb::new(range, matrix),
            },
            PixelFormat::NV12 { range, matrix } => {
                let (luma, chroma) = frame.nv12_planes()?;
                Self::Nv12 {
                    luma,
                    chroma: chroma.as_chunks().0,
                    convert: YuvToRgb::new(range, matrix),
                }
            }
            PixelFormat::MJPEG => return None,
        })
    }

    /// Row `y` of a frame `width` pixels wide; `None` past its last row.
    fn row(self, y: usize, width: usize) -> Option<Self> {
        let pairs = width / 2;
        Some(match self {
            Self::Rgb { pixels, bgr } => Self::Rgb {
                pixels: pixels.get(y * width..(y + 1) * width)?,
                bgr,
            },
            Self::Yuyv {
                pairs: data,
                convert,
            } => Self::Yuyv {
                pairs: data.get(y * pairs..(y + 1) * pairs)?,
                convert,
            },
            Self::Nv12 {
                luma,
                chroma,
                convert,
            } => Self::Nv12 {
                luma: luma.get(y * width..(y + 1) * width)?,
                chroma: chroma.get(y / 2 * pairs..(y / 2 + 1) * pairs)?,
                convert,
            },
        })
    }

    /// Adds the linear light of every `step`th pixel of this row's `span`,
    /// from its first, to `sum`.
    fn add_span(self, sum: &mut [u64; 3], span: Range<usize>, step: usize) {
        if span.is_empty() {
            return;
        }
        // Each format's pixels sliced to the span, so the loop indexes them
        // from 0 without bounds checks; a span may start on a pair's second pixel.
        let (pairs, odd) = (span.start / 2..span.end.div_ceil(2), span.start % 2);
        match self {
            Self::Rgb { pixels, bgr } => {
                let pixels = &pixels[span];
                if bgr {
                    add_linear(sum, pixels.len(), step, |i| {
                        let [b, g, r] = pixels[i];
                        [r, g, b]
                    });
                } else {
                    add_linear(sum, pixels.len(), step, |i| pixels[i]);
                }
            }
            Self::Yuyv {
                pairs: data,
                convert,
            } => {
                let data = &data[pairs];
                add_linear(sum, span.len(), step, |i| {
                    let [y0, u, y1, v] = data[(i + odd) / 2];
                    let luma = if (i + odd) % 2 == 0 { y0 } else { y1 };
                    convert.rgb(luma, u, v).into()
                });
            }
            Self::Nv12 {
                luma,
                chroma,
                convert,
            } => {
                let (luma, chroma) = (&luma[span], &chroma[pairs]);
                add_linear(sum, luma.len(), step, |i| {
                    let [u, v] = chroma[(i + odd) / 2];
                    convert.rgb(luma[i], u, v).into()
                });
            }
        }
    }
}

/// The mean of `pixels` pixels whose linear light sums to `sum`, from
/// [`add_linear`]; black without pixels. The one rounding of the average.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "sums below 2^53 are exact in f64; the mean is in 0..=1"
)]
fn mean(sum: [u64; 3], pixels: usize) -> LinearColor {
    if pixels == 0 {
        return LinearColor::black();
    }
    let scale = pixels as f64 * f64::from(1_u32 << FRACTION_BITS);
    let [r, g, b] = sum.map(|channel| (channel as f64 / scale) as f32);
    LinearColor::new(r, g, b)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use prysm_core::Color;

    /// Build a YUYV frame with neutral chroma from a per-pixel luma function.
    pub fn yuyv_frame_from_luma(width: u32, height: u32, luma: impl Fn(u32, u32) -> u8) -> Frame {
        let mut data = Vec::with_capacity((width * height * 2) as usize);
        for y in 0..height {
            for pair_x in (0..width).step_by(2) {
                data.push(luma(pair_x, y)); // Y0
                data.push(128); // U
                data.push(luma((pair_x + 1).min(width - 1), y)); // Y1
                data.push(128); // V
            }
        }
        Frame::new(
            data,
            width,
            height,
            PixelFormat::YUYV {
                range: prysm_core::YuvRange::Full,
                matrix: prysm_core::YuvStandardMatrix::Bt601,
            },
        )
        .unwrap()
    }

    #[test]
    fn luma_reads_yuyv_directly() {
        let frame = yuyv_frame_from_luma(4, 2, |x, y| u8::try_from(x + y * 10).unwrap() * 10);
        assert_eq!(luma_at(&frame, 0, 0), 0);
        assert_eq!(luma_at(&frame, 1, 0), 10);
        assert_eq!(luma_at(&frame, 3, 1), 130);
        // Out of bounds reads as black
        assert_eq!(luma_at(&frame, 4, 0), 0);
        assert_eq!(luma_at(&frame, 0, 2), 0);
    }

    #[test]
    fn nv12_luma_and_averages_read_the_planes_directly() {
        let format = PixelFormat::NV12 {
            range: prysm_core::YuvRange::Full,
            matrix: prysm_core::YuvStandardMatrix::Bt601,
        };
        // 4x2: left half black, right half white; neutral chroma.
        let frame = Frame::new(
            [0, 0, 255, 255, 0, 0, 255, 255, 128, 128, 128, 128].to_vec(),
            4,
            2,
            format,
        )
        .unwrap();
        assert_eq!(luma_at(&frame, 2, 1), 255);
        assert_eq!(luma_at(&frame, 1, 1), 0);
        assert_eq!(luma_at(&frame, 4, 0), 0);
        let mut view = FrameView::new(frame);
        assert_eq!(
            view.average_linear(0, 0, 4, 2),
            LinearColor::new(0.5, 0.5, 0.5)
        );
        view.viewport.x = 1;
        // Columns 1..4 after the offset, clipped at the frame: one black, two white.
        let avg = view.average_linear(0, 0, 4, 2);
        assert!((avg.r - 2.0 / 3.0).abs() < 1e-6, "{avg:?}");
    }

    #[test]
    fn packed_row_spans_preserve_channel_order_and_clip_to_frame() {
        let rgb = [[2, 30, 60], [255, 0, 0], [21, 39, 53], [0, 0, 0]];
        for format in [PixelFormat::RGB24, PixelFormat::BGR24] {
            let data = rgb
                .iter()
                .flat_map(|&[r, g, b]| {
                    if format == PixelFormat::RGB24 {
                        [r, g, b]
                    } else {
                        [b, g, r]
                    }
                })
                .collect();
            let mut view = FrameView::new(Frame::new(data, 2, 2, format).unwrap());
            view.viewport.x = 1;
            assert_eq!(
                view.average_linear(0, 0, 3, 2),
                LinearColor::new(0.5, 0.0, 0.0)
            );
        }
    }

    #[test]
    fn average_linear_uniform_region() {
        let frame = yuyv_frame_from_luma(8, 8, |_, _| 128);
        let view = FrameView::new(frame);
        let avg = view.average_linear(0, 0, 8, 8);
        let expected = LinearColor::from_srgb(Color::new(128, 128, 128));
        assert!((avg.r - expected.r).abs() < 0.01, "r = {}", avg.r);
        assert!((avg.g - expected.g).abs() < 0.01);
        assert!((avg.b - expected.b).abs() < 0.01);
    }

    #[test]
    fn average_linear_respects_viewport_offset_and_keeps_black_in_the_average() {
        let frame = yuyv_frame_from_luma(8, 8, |x, y| {
            if (2..6).contains(&x) && y == 2 {
                255
            } else {
                0
            }
        });
        let mut view = FrameView::new(frame);
        view.viewport = Viewport {
            x: 2,
            y: 2,
            width: 4,
            height: 4,
        };
        let avg = view.average_linear(0, 0, 4, 2);
        // Both rows count equally, including black pixels.
        assert!(
            (avg.r - 0.5).abs() < 1e-6,
            "expected 50% white, got {}",
            avg.r
        );
    }

    /// The cell means against an `f64` reference that reads each lattice pixel
    /// on its own: its 8-bit RGB from `prysm_core`'s per-pixel views (the
    /// fixed-point conversion, checked against the standards there), the
    /// exact IEC 61966-2-1 decoding, and an `f64` mean. Integer sums are
    /// exact, so the only error left is the table's rounding to 2^-21 and the
    /// final `f32`: within 1e-6 everywhere.
    #[test]
    fn cell_means_match_an_exact_f64_reference_for_every_format_step_and_alignment() {
        use prysm_core::nv12::Nv12View;
        use prysm_core::yuyv::YuyvView;
        use prysm_core::{YuvRange, YuvStandardMatrix};
        fn decode(code: u8) -> f64 {
            let srgb = f64::from(code) / 255.0;
            if srgb <= 0.04045 {
                srgb / 12.92
            } else {
                ((srgb + 0.055) / 1.055).powf(2.4)
            }
        }
        let mut seed = 0x9E37_79B9_u32;
        let mut noise = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let (width, height) = (46, 30);
        let formats = [
            PixelFormat::RGB24,
            PixelFormat::BGR24,
            PixelFormat::YUYV {
                range: YuvRange::Full,
                matrix: YuvStandardMatrix::Bt601,
            },
            PixelFormat::YUYV {
                range: YuvRange::Limited,
                matrix: YuvStandardMatrix::Bt2020,
            },
            PixelFormat::NV12 {
                range: YuvRange::Limited,
                matrix: YuvStandardMatrix::Bt709,
            },
            PixelFormat::NV12 {
                range: YuvRange::Full,
                matrix: YuvStandardMatrix::Smpte240,
            },
        ];
        let mut worst = 0.0_f64;
        for format in formats {
            let size = format.expected_size(width, height).unwrap().unwrap();
            // Noise, and a bright and a dark block so the means span the range.
            let data = (0..size)
                .map(|i| match i * 8 / size {
                    2 => 255,
                    5 => 0,
                    _ => noise().to_le_bytes()[0],
                })
                .collect();
            let frame = Frame::new(data, width, height, format).unwrap();
            let (nv12, yuyv) = (Nv12View::new(&frame), YuyvView::new(&frame));
            let pixel = |x: u32, y: u32| -> [u8; 3] {
                let index = ((y * width + x) * 3) as usize;
                match (format, nv12, yuyv) {
                    (_, Some(view), _) => view.pixel(x, y).into(),
                    (_, _, Some(view)) => view.pixel(x, y).into(),
                    (PixelFormat::BGR24, ..) => {
                        let [b, g, r] = frame.as_slice()[index..index + 3] else {
                            unreachable!()
                        };
                        [r, g, b]
                    }
                    _ => frame.as_slice()[index..index + 3].try_into().unwrap(),
                }
            };
            let mut view = FrameView::new(frame.clone());
            for (vx, vy) in [(0, 0), (1, 3), (5, 2)] {
                view.viewport = Viewport {
                    x: vx,
                    y: vy,
                    width: width - vx,
                    height: height - vy,
                };
                // Edges at odd and even columns, a cell past the frame's edge,
                // and an empty one.
                let x_edges = [0, 1, 4, 9, 9, 16, 27, 38, 47, 60];
                for (rows, step) in [(0..30, 1), (3..17, 2), (1..28, 3), (2..25, 5), (7..8, 4)] {
                    let mut cells = [LinearColor::black(); 9];
                    view.average_cells(rows.clone(), &x_edges, step, &mut cells);
                    for (cell, edge) in cells.iter().zip(x_edges.windows(2)) {
                        let clip =
                            |origin: u32, offset: u32, limit: u32| (origin + offset).min(limit);
                        let xs = clip(vx, edge[0], width)..clip(vx, edge[1], width);
                        let ys = clip(vy, rows.start, height)..clip(vy, rows.end, height);
                        let (mut sum, mut count) = ([0.0_f64; 3], 0_u32);
                        for y in ys.step_by(step as usize) {
                            for x in xs.clone().step_by(step as usize) {
                                for (sum, code) in sum.iter_mut().zip(pixel(x, y)) {
                                    *sum += decode(code);
                                }
                                count += 1;
                            }
                        }
                        let expected = sum.map(|sum| {
                            if count == 0 {
                                0.0
                            } else {
                                sum / f64::from(count)
                            }
                        });
                        for (actual, expected) in [cell.r, cell.g, cell.b].into_iter().zip(expected)
                        {
                            let error = (f64::from(actual) - expected).abs();
                            worst = worst.max(error);
                            assert!(
                                error <= 1e-6,
                                "{format:?} at ({vx}, {vy}), step {step}, {edge:?}: {actual} vs {expected}"
                            );
                        }
                    }
                }
            }
        }
        // The bound is met with room, not by luck of the frames.
        assert!(worst > 1e-8 && worst <= 5.2e-7, "worst error {worst}");
    }
}
