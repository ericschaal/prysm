use super::PixelView;
use prysm_core::{Frame, LinearColor};

pub use prysm_core::Viewport;

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
        self.average_linear_every(x_start, y_start, x_end, y_end, 1)
    }

    /// Like [`Self::average_linear`], but reads only every `step`th pixel of
    /// every `step`th row, starting at the region's first pixel, so a region
    /// always contributes at least one pixel. `step` 1 reads every pixel. The
    /// region is clipped to the frame.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "accumulate in f64, then return the bounded mean in the f32 color representation"
    )]
    pub fn average_linear_every(
        &self,
        x_start: u32,
        y_start: u32,
        x_end: u32,
        y_end: u32,
        step: u32,
    ) -> LinearColor {
        let width = self.frame.width();
        let height = self.frame.height();
        let x0 = self.viewport.x.saturating_add(x_start).min(width);
        let x1 = self.viewport.x.saturating_add(x_end).min(width);
        let y0 = self.viewport.y.saturating_add(y_start).min(height);
        let y1 = self.viewport.y.saturating_add(y_end).min(height);
        if x0 >= x1 || y0 >= y1 {
            return LinearColor::black();
        }
        let pixels = PixelView::new(&self.frame);
        let step = step.max(1);
        let mut sum = [0.0f64; 3];
        let mut count = 0u64;
        for y in (y0..y1).step_by(step as usize) {
            let xs = (x0 as usize..x1 as usize).step_by(step as usize);
            pixels.for_each_rgb(y, xs, |color| {
                let linear = LinearColor::from_srgb(color);
                sum[0] += f64::from(linear.r);
                sum[1] += f64::from(linear.g);
                sum[2] += f64::from(linear.b);
                count += 1;
            });
        }
        let [r, g, b] = sum.map(|channel| (channel / count as f64) as f32);
        LinearColor::new(r, g, b)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use prysm_core::{Color, PixelFormat};

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
    fn detection_reads_yuyv_luma_and_chroma_directly() {
        let frame = yuyv_frame_from_luma(4, 2, |x, y| u8::try_from(x + y * 10).unwrap() * 10);
        let pixels = PixelView::new(&frame);
        assert_eq!(pixels.luma_and_chroma(0, 0), (0, 0));
        assert_eq!(pixels.luma_and_chroma(1, 0), (10, 0));
        assert_eq!(pixels.luma_and_chroma(3, 1), (130, 0));
        let rgb = Frame::new(vec![10, 200, 30, 0, 0, 0], 2, 1, PixelFormat::BGR24).unwrap();
        assert_eq!(PixelView::new(&rgb).luma_and_chroma(0, 0), (200, 0));
    }

    #[test]
    #[should_panic(expected = "index out of bounds")]
    fn a_pixel_right_of_the_frame_is_a_bug_not_the_next_row() {
        let frame = yuyv_frame_from_luma(4, 2, |_, _| 0);
        let _ = PixelView::new(&frame).luma_and_chroma(4, 0);
    }

    #[test]
    fn nv12_luma_and_averages_read_the_planes_directly() {
        let format = PixelFormat::NV12 {
            range: prysm_core::YuvRange::Full,
            matrix: prysm_core::YuvStandardMatrix::Bt601,
        };
        // 4x2: left half black, right half white; chroma neutral on the left, red on the right.
        let frame = Frame::new(
            [0, 0, 255, 255, 0, 0, 255, 255, 128, 128, 128, 200].to_vec(),
            4,
            2,
            format,
        )
        .unwrap();
        let pixels = PixelView::new(&frame);
        assert_eq!(pixels.luma_and_chroma(2, 1), (255, 72));
        assert_eq!(pixels.luma_and_chroma(1, 1), (0, 0));
        let frame = Frame::new(
            [0, 0, 255, 255, 0, 0, 255, 255, 128, 128, 128, 128].to_vec(),
            4,
            2,
            format,
        )
        .unwrap();
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
        assert!(
            (avg.r - 0.5).abs() < 1e-6,
            "expected 50% white, got {}",
            avg.r
        );
    }

    /// The cell means against an `f64` reference that reads each lattice pixel
    /// on its own: its 8-bit RGB from `prysm_core`'s per-pixel views (the
    /// fixed-point conversion, checked against the standards there), the
    /// exact IEC 61966-2-1 decoding, and an `f64` mean. Within 1e-6 everywhere.
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
            let size = format.expected_size(width, height).unwrap();
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
                    for edge in x_edges.windows(2) {
                        let cell =
                            view.average_linear_every(edge[0], rows.start, edge[1], rows.end, step);
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
        assert!(worst > 0.0, "worst error {worst}");
    }
}
