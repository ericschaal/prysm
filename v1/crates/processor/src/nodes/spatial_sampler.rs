use crate::frames::FrameView;
use prysm_core::{ColorStrip, Edge, EdgeColors, LinearColor, PixelFormat, SampleDensity};

/// Most pixels cell averaging reads per frame: every pixel of a 640x360
/// picture. Larger pictures are read on a lattice of every `step`th pixel and
/// row, so sampling costs about the same at any capture size.
pub(crate) const PIXEL_BUDGET: u64 = 640 * 360;

/// The smallest lattice step that reads at most [`PIXEL_BUDGET`] pixels of a
/// `width` x `height` picture.
pub(crate) fn lattice_step(width: u32, height: u32) -> u32 {
    let mut step = 1;
    while u64::from(width.div_ceil(step)) * u64::from(height.div_ceil(step)) > PIXEL_BUDGET {
        step += 1;
    }
    step
}

/// Projects the whole picture onto the perimeter using a smooth distance falloff.
#[derive(Debug)]
pub struct SpatialSampler {
    sample_density: SampleDensity,
    reach_ratio: f32,
    dimensions: Option<(u32, u32)>,
    step: u32,
    cells: Vec<[u32; 4]>,
    cell_colors: Vec<LinearColor>,
    rgb: Vec<u8>,
    // One row of normalized cell weights per output sample, in strip order.
    sample_weights: [Vec<f32>; 4],
}

/// Samples projected together, so each cell color is loaded once per group.
const GROUP: usize = 4;
/// Cells summed in `f32` before the partial sum is added in `f64`: keeps the
/// rounding error of a projected sample below about 1e-6 of its value.
const BLOCK: usize = 64;

/// Weighted sums of `colors` for up to [`GROUP`] samples, one weight row per
/// sample in `rows`.
#[allow(
    clippy::cast_possible_truncation,
    reason = "normalized weighted channel means fit the f32 color representation"
)]
fn project(colors: &[LinearColor], rows: &[f32], out: &mut [LinearColor]) {
    let cells = colors.len();
    // A short last group repeats its first row; the extra sums are discarded.
    let row = |sample: usize| {
        let start = if sample < out.len() {
            sample * cells
        } else {
            0
        };
        &rows[start..start + cells]
    };
    let rows: [&[f32]; GROUP] = std::array::from_fn(row);
    let mut total = [[0.0_f64; 3]; GROUP];
    for start in (0..cells).step_by(BLOCK) {
        let end = (start + BLOCK).min(cells);
        let mut partial = [[0.0_f32; 3]; GROUP];
        let weights = rows.map(|row| &row[start..end]);
        for (cell, color) in colors[start..end].iter().enumerate() {
            for (sums, weights) in partial.iter_mut().zip(weights) {
                let weight = weights[cell];
                sums[0] += color.r * weight;
                sums[1] += color.g * weight;
                sums[2] += color.b * weight;
            }
        }
        for (total, partial) in total.iter_mut().zip(partial) {
            for (total, partial) in total.iter_mut().zip(partial) {
                *total += f64::from(partial);
            }
        }
    }
    for (out, [r, g, b]) in out.iter_mut().zip(total) {
        *out = LinearColor::new(r as f32, g as f32, b as f32);
    }
}

impl SpatialSampler {
    pub fn new(sample_density: SampleDensity, reach_ratio: f32) -> Self {
        Self {
            sample_density,
            reach_ratio,
            dimensions: None,
            step: 1,
            cells: Vec::new(),
            cell_colors: Vec::new(),
            rgb: Vec::new(),
            sample_weights: std::array::from_fn(|_| Vec::new()),
        }
    }

    #[allow(
        clippy::float_cmp,
        reason = "invalidate geometry on any change to the configured reach"
    )]
    pub fn configure(&mut self, sample_density: SampleDensity, reach_ratio: f32) {
        if self.sample_density != sample_density || self.reach_ratio != reach_ratio {
            self.sample_density = sample_density;
            self.reach_ratio = reach_ratio;
            self.dimensions = None;
        }
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        reason = "grid coordinates are bounded by u32 dimensions; weights use approximate f32 geometry"
    )]
    fn prepare_geometry(&mut self, width: u32, height: u32) {
        let horizontal = self.sample_density.samples_for_length(width as usize);
        let vertical = self.sample_density.samples_for_length(height as usize);
        // A picture larger than the budget gets the grid of its copy scaled
        // down to the budget, so cells hold about as many read pixels, and
        // projection costs about the same, at any capture size.
        let pixels = u64::from(width) * u64::from(height);
        let budget_length = if pixels > PIXEL_BUDGET {
            (f64::from(width.max(height)) * (PIXEL_BUDGET as f64 / pixels as f64).sqrt()).round()
                as usize
        } else {
            width.max(height) as usize
        };
        // shortcut: cap the grid at 128 cells per axis; raise only if footage shows lost detail.
        let longest = self
            .sample_density
            .samples_for_length(budget_length)
            .clamp(64, 128) as u32;
        let grid_length = |length: u32| {
            (u64::from(length) * u64::from(longest) / u64::from(width.max(height)))
                .clamp(1, u64::from(length)) as u32
        };
        let (columns, rows) = (grid_length(width), grid_length(height));
        self.cells.clear();
        for y in 0..rows {
            for x in 0..columns {
                self.cells.push([
                    (u64::from(x) * u64::from(width) / u64::from(columns)) as u32,
                    (u64::from(y) * u64::from(height) / u64::from(rows)) as u32,
                    (u64::from(x + 1) * u64::from(width) / u64::from(columns)) as u32,
                    (u64::from(y + 1) * u64::from(height) / u64::from(rows)) as u32,
                ]);
            }
        }
        self.cell_colors
            .resize(self.cells.len(), LinearColor::black());
        let radius = height as f32 * self.reach_ratio;
        for (weights, (edge, count)) in self.sample_weights.iter_mut().zip([
            (Edge::Top, horizontal),
            (Edge::Right, vertical),
            (Edge::Bottom, horizontal),
            (Edge::Left, vertical),
        ]) {
            weights.clear();
            for i in 0..count {
                let position = (i as f32 + 0.5) / count as f32;
                // Preserve the existing left-to-right / top-to-bottom strip coordinates.
                let (sample_x, sample_y) = match edge {
                    Edge::Top => (position * width as f32, 0.0),
                    Edge::Right => (width as f32, position * height as f32),
                    Edge::Bottom => (position * width as f32, height as f32),
                    Edge::Left => (0.0, position * height as f32),
                };
                let start = weights.len();
                for &[x0, y0, x1, y1] in &self.cells {
                    let dx = ((x0 as f32).midpoint(x1 as f32) - sample_x) / radius;
                    let dy = ((y0 as f32).midpoint(y1 as f32) - sample_y) / radius;
                    let area = (x1 - x0) as f32 * (y1 - y0) as f32;
                    let distance_squared = dx * dx + dy * dy;
                    // A peak at radius/4 restores local contrast; the broad tail carries the scene.
                    let local = 1.0 + 16.0 * distance_squared;
                    weights.push(area * (1.0 / (1.0 + distance_squared) + 64.0 / (local * local)));
                }
                // Geometry-only normalization keeps black and small highlights proportional.
                let total: f64 = weights[start..].iter().map(|&w| f64::from(w)).sum();
                for weight in &mut weights[start..] {
                    *weight = (f64::from(*weight) / total) as f32;
                }
            }
        }
        self.step = lattice_step(width, height);
        self.dimensions = Some((width, height));
    }

    #[allow(
        clippy::cast_possible_truncation,
        reason = "normalized weighted channel means fit the f32 color representation"
    )]
    pub fn process(&mut self, view: &FrameView) -> EdgeColors {
        let (width, height) = (view.viewport_width(), view.viewport_height());
        if width == 0 || height == 0 {
            return EdgeColors::black(width as usize, height as usize, self.sample_density);
        }
        if self.dimensions != Some((width, height)) {
            self.prepare_geometry(width, height);
        }
        // shortcut: full-frame SIMD is slower for tiny crops; revisit if they are common in real use.
        let rgb = if matches!(view.frame.format(), PixelFormat::YUYV { .. }) {
            if let Err(error) = prysm_core::yuyv::yuyv_to_rgb_into(&view.frame, &mut self.rgb) {
                tracing::error!(%error, "Cannot convert sampling frame");
                return EdgeColors::black(width as usize, height as usize, self.sample_density);
            }
            Some(self.rgb.as_slice())
        } else {
            None
        };
        let cells = self.cell_colors.iter_mut().zip(&self.cells);
        let step = self.step;
        cells.for_each(|(color, &[x0, y0, x1, y1])| {
            *color = view.average_linear_every(x0, y0, x1, y1, rgb, step);
        });
        let [top, right, bottom, left] = self.sample_weights.each_ref().map(|weights| {
            let cell_count = self.cell_colors.len();
            let mut samples = vec![LinearColor::black(); weights.len() / cell_count];
            let groups = samples
                .chunks_mut(GROUP)
                .zip(weights.chunks(GROUP * cell_count));
            groups.for_each(|(out, rows)| project(&self.cell_colors, rows, out));
            ColorStrip::new(samples).expect("sample count is nonzero")
        });
        EdgeColors::new(top, right, bottom, left)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::Viewport;
    use prysm_core::Color;
    use prysm_core::{Frame, PixelFormat};

    fn picture(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> FrameView {
        let mut data = Vec::new();
        for y in 0..height {
            for x in 0..width {
                data.extend(pixel(x, y));
            }
        }
        FrameView::new(Frame::new(data, width, height, PixelFormat::RGB24).unwrap())
    }

    fn strips(colors: &EdgeColors) -> [&ColorStrip; 4] {
        [&colors.top, &colors.right, &colors.bottom, &colors.left]
    }

    #[test]
    fn yuyv_conversion_reuses_storage_and_matches_rgb_with_an_offset_crop() {
        use prysm_core::{YuvRange, YuvStandardMatrix};
        let mut sampler = SpatialSampler::new(SampleDensity(128), 0.1);
        for range in [YuvRange::Full, YuvRange::Limited] {
            for matrix in [
                YuvStandardMatrix::Bt601,
                YuvStandardMatrix::Bt709,
                YuvStandardMatrix::Bt2020,
                YuvStandardMatrix::Smpte240,
            ] {
                for seed in [13_u8, 91] {
                    let data = (0..66 * 34 * 2)
                        .map(|i: usize| i.to_le_bytes()[0].wrapping_add(seed))
                        .collect();
                    let mut raw = FrameView::new(
                        Frame::new(data, 66, 34, PixelFormat::YUYV { range, matrix }).unwrap(),
                    );
                    raw.viewport = Viewport {
                        x: 3,
                        y: 5,
                        width: 45,
                        height: 19,
                    };
                    let mut rgb = FrameView::new(
                        Frame::new(
                            prysm_core::yuyv::yuyv_to_rgb(&raw.frame).unwrap(),
                            66,
                            34,
                            PixelFormat::RGB24,
                        )
                        .unwrap(),
                    );
                    rgb.viewport = raw.viewport;
                    let actual = sampler.process(&raw);
                    let allocation = sampler.rgb.as_ptr();
                    let expected = sampler.process(&rgb);
                    assert_eq!(actual, expected, "{range:?} {matrix:?}");
                    assert_eq!(sampler.process(&raw), expected);
                    assert_eq!(sampler.rgb.as_ptr(), allocation);
                }
            }
        }
    }

    #[test]
    fn nv12_is_sampled_in_place_and_matches_its_rgb_conversion_with_an_offset_crop() {
        use prysm_core::{YuvRange, YuvStandardMatrix};
        let mut sampler = SpatialSampler::new(SampleDensity(128), 0.1);
        for range in [YuvRange::Full, YuvRange::Limited] {
            for matrix in [YuvStandardMatrix::Bt601, YuvStandardMatrix::Bt709] {
                // Legal codes, where the reference converter and ours agree within one step.
                let data = (0..66 * 34 * 3 / 2)
                    .map(|i: usize| 16 + u8::try_from(i * 7 % 220).unwrap())
                    .collect();
                let mut raw = FrameView::new(
                    Frame::new(data, 66, 34, PixelFormat::NV12 { range, matrix }).unwrap(),
                );
                raw.viewport = Viewport {
                    x: 3,
                    y: 5,
                    width: 45,
                    height: 19,
                };
                let mut rgb = FrameView::new(
                    Frame::new(
                        prysm_core::nv12::nv12_to_rgb(&raw.frame).unwrap(),
                        66,
                        34,
                        PixelFormat::RGB24,
                    )
                    .unwrap(),
                );
                rgb.viewport = raw.viewport;
                let actual = sampler.process(&raw);
                assert!(sampler.rgb.is_empty(), "NV12 must not be converted");
                let expected = sampler.process(&rgb);
                for (actual, expected) in strips(&actual).into_iter().zip(strips(&expected)) {
                    for (a, e) in actual
                        .resample(actual.len())
                        .into_iter()
                        .zip(expected.resample(expected.len()))
                    {
                        for (a, e) in [a.r, a.g, a.b].into_iter().zip([e.r, e.g, e.b]) {
                            assert!((a - e).abs() < 0.01, "{range:?} {matrix:?}: {a} != {e}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn projected_samples_match_serial_sums_in_strip_order() {
        let mut sampler = SpatialSampler::new(SampleDensity(700), 0.15);
        let mut view = picture(93, 55, |x, y| {
            [
                x.to_le_bytes()[0],
                y.to_le_bytes()[0],
                (x ^ y).to_le_bytes()[0],
            ]
        });
        view.viewport = Viewport {
            x: 3,
            y: 5,
            width: 86,
            height: 46,
        };
        let output = sampler.process(&view);
        let cells: Vec<_> = sampler
            .cells
            .iter()
            .map(|&[x0, y0, x1, y1]| view.average_linear(x0, y0, x1, y1, None))
            .collect();
        assert_eq!(sampler.cell_colors, cells);
        for (strip, weights) in strips(&output).into_iter().zip(&sampler.sample_weights) {
            let reference: Vec<_> = weights
                .chunks_exact(cells.len())
                .map(|row| {
                    let sum = |channel: fn(&LinearColor) -> f32| {
                        #[allow(clippy::cast_possible_truncation)]
                        let value = cells
                            .iter()
                            .zip(row)
                            .map(|(cell, w)| f64::from(channel(cell)) * f64::from(*w))
                            .sum::<f64>() as f32;
                        value
                    };
                    LinearColor::new(sum(|c| c.r), sum(|c| c.g), sum(|c| c.b))
                })
                .collect();
            // Blocks of cells are summed in f32, so the sums agree to rounding.
            for (actual, expected) in strip.resample(strip.len()).into_iter().zip(reference) {
                for (a, e) in [actual.r, actual.g, actual.b]
                    .into_iter()
                    .zip([expected.r, expected.g, expected.b])
                {
                    assert!((a - e).abs() <= e.abs() * 1e-6, "{a} != {e}");
                }
            }
        }
    }

    #[test]
    fn lattice_reads_every_pixel_up_to_the_budget_and_about_the_budget_beyond() {
        assert_eq!(lattice_step(1, 1), 1);
        assert_eq!(lattice_step(640, 360), 1);
        assert_eq!(lattice_step(360, 640), 1);
        assert_eq!(lattice_step(642, 360), 2);
        assert_eq!(lattice_step(960, 540), 2);
        assert_eq!(lattice_step(1280, 720), 2);
        assert_eq!(lattice_step(1920, 1080), 3);
        assert_eq!(lattice_step(3840, 2160), 6);
        for (width, height) in [
            (642, 360),
            (1280, 720),
            (1281, 721),
            (1920, 1080),
            (5000, 7),
        ] {
            let step = lattice_step(width, height);
            let read =
                |step: u32| u64::from(width.div_ceil(step)) * u64::from(height.div_ceil(step));
            assert!(read(step) <= PIXEL_BUDGET);
            assert!(
                step == 1 || read(step - 1) > PIXEL_BUDGET,
                "{width}x{height}: {step}"
            );
        }
    }

    #[test]
    fn large_pictures_are_read_on_the_lattice_and_use_the_budget_sized_grid() {
        let mut sampler = SpatialSampler::new(SampleDensity(128), 0.1);
        let _ = sampler.process(&picture(1280, 720, |_, _| [0; 3]));
        assert_eq!(sampler.step, 2);
        let mut small = SpatialSampler::new(SampleDensity(128), 0.1);
        let _ = small.process(&picture(640, 360, |_, _| [0; 3]));
        assert_eq!(small.step, 1);
        assert_eq!(
            sampler.cells.len(),
            small.cells.len(),
            "the grid of 640x360"
        );
        // Each cell averages exactly its lattice pixels.
        let view = picture(1280, 720, |x, y| {
            [
                (x * 7).to_le_bytes()[0],
                (y * 3).to_le_bytes()[0],
                (x ^ y).to_le_bytes()[0],
            ]
        });
        let _ = sampler.process(&view);
        for (&[x0, y0, x1, y1], &actual) in sampler.cells.iter().zip(&sampler.cell_colors) {
            let mut sum = [0.0_f64; 3];
            let mut count = 0.0;
            for y in (y0..y1).step_by(2) {
                for x in (x0..x1).step_by(2) {
                    let pixel = view.average_linear(x, y, x + 1, y + 1, None);
                    for (sum, value) in sum.iter_mut().zip([pixel.r, pixel.g, pixel.b]) {
                        *sum += f64::from(value);
                    }
                    count += 1.0;
                }
            }
            #[allow(clippy::cast_possible_truncation)]
            let expected = sum.map(|channel| (channel / count) as f32);
            assert_eq!([actual.r, actual.g, actual.b], expected);
        }
    }

    #[test]
    fn a_doubled_picture_lights_exactly_like_the_original() {
        // The lattice reads one pixel of each 2x2 block of a doubled picture.
        // Half the density gives both the same samples and the same 64-cell grid.
        let pixel = |x: u32, y: u32| {
            if x < 200 && y < 180 {
                [255, 64, 0]
            } else if x > 500 {
                [0, 90, 255]
            } else {
                [(x / 3).to_le_bytes()[0], 30, (y / 2).to_le_bytes()[0]]
            }
        };
        let original =
            SpatialSampler::new(SampleDensity(100), 0.1).process(&picture(640, 360, pixel));
        let doubled =
            SpatialSampler::new(SampleDensity(50), 0.1)
                .process(&picture(1280, 720, |x, y| pixel(x / 2, y / 2)));
        assert_eq!(doubled, original);
    }

    #[test]
    fn small_highlights_on_the_lattice_still_reach_the_edges() {
        let mut sampler = SpatialSampler::new(SampleDensity(128), 0.1);
        let output = sampler.process(&picture(1280, 720, |x, y| {
            if (638..640).contains(&x) && (358..360).contains(&y) {
                [255, 0, 0]
            } else {
                [0, 0, 0]
            }
        }));
        for strip in strips(&output) {
            assert!(strip.sample_at(0.5).r > 0.0);
        }
    }

    #[test]
    fn uniform_colors_and_black_are_preserved_at_every_density() {
        for density in [0, 128, 2000] {
            for (width, height) in [(1, 1), (1, 8), (8, 1), (8, 8), (133, 79)] {
                let mut sampler = SpatialSampler::new(SampleDensity(density), 0.1);
                for rgb in [[0, 0, 0], [255, 255, 255], [64, 128, 192]] {
                    let expected = LinearColor::from_srgb(Color::new(rgb[0], rgb[1], rgb[2]));
                    let output = sampler.process(&picture(width, height, |_, _| rgb));
                    for strip in strips(&output) {
                        for actual in strip.resample(strip.len()) {
                            for (a, b) in [actual.r, actual.g, actual.b]
                                .into_iter()
                                .zip([expected.r, expected.g, expected.b])
                            {
                                assert!((a - b).abs() < 1e-6, "{actual:?} != {expected:?}");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn center_light_reaches_every_edge_and_moves_toward_the_left() {
        let mut sampler = SpatialSampler::new(SampleDensity(250), 0.1);
        let mut previous = 0.0;
        for x0 in [30, 22, 14, 6, 0] {
            let output = sampler.process(&picture(64, 64, |x, y| {
                if (x0..x0 + 4).contains(&x) && (30..34).contains(&y) {
                    [255, 128, 0]
                } else {
                    [0, 0, 0]
                }
            }));
            for strip in strips(&output) {
                for color in strip.resample(strip.len()) {
                    assert!(color.r > color.g && color.g > 0.0 && color.b == 0.0);
                    let ceiling = if x0 == 30 { 0.02 } else { 0.6 };
                    assert!(
                        color.r < ceiling,
                        "small light overpowered the scene: {color:?}"
                    );
                }
            }
            let left = output.left.sample_at(0.5).r;
            let right = output.right.sample_at(0.5).r;
            if x0 == 30 {
                assert!((left - right).abs() < 1e-6);
            } else {
                assert!(left > right);
            }
            assert!(left > previous);
            assert!(left > output.left.sample_at(0.0).r);
            previous = left;
        }
    }

    #[test]
    fn opposing_regions_keep_their_colors_and_strip_orientation() {
        let mut sampler = SpatialSampler::new(SampleDensity(250), 0.1);
        for vertical in [false, true] {
            let output = sampler.process(&picture(64, 64, |x, y| {
                if (if vertical { y } else { x }) < 32 {
                    [255, 0, 0]
                } else {
                    [0, 0, 255]
                }
            }));
            let (red, blue, transitions) = if vertical {
                (&output.top, &output.bottom, [&output.left, &output.right])
            } else {
                (&output.left, &output.right, [&output.top, &output.bottom])
            };
            assert!(red.sample_at(0.5).r > 0.75);
            assert!(blue.sample_at(0.5).b > 0.75);
            for strip in transitions {
                let values = strip.resample(strip.len());
                for (i, color) in values.iter().enumerate() {
                    assert_eq!(color.r > color.b, i < values.len() / 2);
                }
                // Adjacent outputs must transition gradually, including near the corners.
                assert!(
                    strip
                        .resample(128)
                        .windows(2)
                        .all(|p| (p[0].r - p[1].r).abs() < 0.1)
                );
                for color in values {
                    assert!((color.r + color.b - 1.0).abs() < 1e-6);
                }
            }
        }
    }

    #[test]
    fn subcell_highlights_are_not_skipped_and_larger_lights_are_brighter() {
        let mut sampler = SpatialSampler::new(SampleDensity(128), 0.1);
        let small = sampler.process(&picture(640, 360, |x, y| {
            if x == 319 && y == 179 {
                [255, 0, 0]
            } else {
                [0, 0, 0]
            }
        }));
        let large = sampler.process(&picture(640, 360, |x, y| {
            if (318..322).contains(&x) && (178..182).contains(&y) {
                [255, 0, 0]
            } else {
                [0, 0, 0]
            }
        }));
        for (small, large) in strips(&small).into_iter().zip(strips(&large)) {
            assert!(small.sample_at(0.5).r > 0.0);
            assert!(large.sample_at(0.5).r > small.sample_at(0.5).r * 10.0);
        }
    }

    #[test]
    fn cached_geometry_tracks_size_and_reads_current_viewport_offset() {
        let mut sampler = SpatialSampler::new(SampleDensity(128), 0.1);
        for (width, height, offset) in [(33, 17, 0), (33, 17, 11), (17, 33, 5)] {
            let pixel = |x, y| {
                if x < width / 2 && y < height / 2 {
                    [255, 0, 0]
                } else {
                    [0, 0, 128]
                }
            };
            let mut padded = picture(width + offset * 2, height + offset * 2, |x, y| {
                if x >= offset && y >= offset && x < width + offset && y < height + offset {
                    pixel(x - offset, y - offset)
                } else {
                    [0, 255, 0]
                }
            });
            padded.viewport = Viewport {
                x: offset,
                y: offset,
                width,
                height,
            };
            let actual = sampler.process(&padded);
            let expected = SpatialSampler::new(SampleDensity(128), 0.1)
                .process(&picture(width, height, pixel));
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn scaling_the_picture_preserves_influence_in_landscape_and_portrait() {
        for (width, height) in [(64, 36), (36, 64)] {
            let mut sampler = SpatialSampler::new(SampleDensity(0), 0.1);
            let render = |scale| {
                picture(width * scale, height * scale, |x, y| {
                    if x / scale < width / 3 && y / scale < height / 2 {
                        [255, 64, 0]
                    } else {
                        [0, 0, 0]
                    }
                })
            };
            let expected = sampler.process(&render(1));
            for scale in [2, 5] {
                let actual = sampler.process(&render(scale));
                for (actual, expected) in strips(&actual).into_iter().zip(strips(&expected)) {
                    assert!((actual.sample_at(0.5).r - expected.sample_at(0.5).r).abs() < 1e-6);
                }
            }
        }
    }
}
