use crate::Stage;
use crate::frames::FrameView;
use crate::workers::Workers;
use color_focus::ColorFocus;
use prysm_core::{
    ColorStrip, Config, Edge, EdgeColors, LinearColor, PIXEL_BUDGET, SampleDensity,
    SpatialReachPercent,
};

mod color_focus;

/// The smallest lattice step that reads at most [`PIXEL_BUDGET`] pixels of a
/// `width` x `height` picture: cell averaging reads every pixel of a 640x360
/// picture, and larger ones on a lattice of every `step`th pixel and row.
pub(crate) fn lattice_step(width: u32, height: u32) -> u32 {
    let mut step = 1;
    while u64::from(width.div_ceil(step)) * u64::from(height.div_ceil(step)) > PIXEL_BUDGET {
        step += 1;
    }
    step
}

/// Projects the whole picture onto the perimeter using a smooth distance
/// falloff, then optionally lets each sample keep the colour next to it
/// ([`ColorFocus`]). It reads its settings from the [`Config`] of each frame
/// and keeps only what they and the picture size determine.
#[derive(Debug, Default)]
pub struct SpatialSampler {
    geometry: Geometry,
    cell_colors: Vec<LinearColor>,
    focus: ColorFocus,
    // Every edge's output samples in strip order, reused from frame to frame.
    samples: Vec<LinearColor>,
}

/// Everything the sampler's geometry depends on.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct GeometryKey {
    width: u32,
    height: u32,
    density: SampleDensity,
    reach: SpatialReachPercent,
}

impl GeometryKey {
    /// Positions in the same top, right, bottom, left order as the output strips.
    fn sample_points(self, counts: [usize; 4]) -> impl Iterator<Item = [f32; 2]> {
        [Edge::Top, Edge::Right, Edge::Bottom, Edge::Left]
            .into_iter()
            .zip(counts)
            .flat_map(move |(edge, count)| {
                (0..count).map(move |i| sample_point(edge, i, count, self.width, self.height))
            })
    }
}

/// Cells in row order, and one contiguous row of weights for every sample
/// along the perimeter (top, right, bottom, left).
#[derive(Debug, Default)]
struct Geometry {
    key: GeometryKey,
    step: u32,
    cells: Vec<[u32; 4]>,
    columns: usize,
    radius: f32,
    counts: [usize; 4],
    weights: Vec<f32>,
}

/// The kernel's broad tail at a squared distance in radii: it carries the scene.
fn broad_tail(distance_squared: f32) -> f32 {
    1.0 / (1.0 + distance_squared)
}

/// The kernel's local lobe, a peak at a quarter radius: it restores local contrast.
fn local_lobe(distance_squared: f32) -> f32 {
    let local = 1.0 + 16.0 * distance_squared;
    64.0 / (local * local)
}

/// Squared distance from a cell's centre to `point`, in kernel radii.
#[allow(
    clippy::cast_precision_loss,
    reason = "cell coordinates are bounded by u32 dimensions; weights use approximate f32 geometry"
)]
fn distance_squared([x0, y0, x1, y1]: [u32; 4], [x, y]: [f32; 2], radius: f32) -> f32 {
    let dx = ((x0 as f32).midpoint(x1 as f32) - x) / radius;
    let dy = ((y0 as f32).midpoint(y1 as f32) - y) / radius;
    dx * dx + dy * dy
}

#[allow(
    clippy::cast_precision_loss,
    reason = "cell sides are bounded by u32 dimensions; weights use approximate f32 geometry"
)]
fn cell_area([x0, y0, x1, y1]: [u32; 4]) -> f32 {
    (x1 - x0) as f32 * (y1 - y0) as f32
}

/// Where sample `index` of `count` sits on `edge` of a `width` x `height`
/// picture, keeping the left-to-right / top-to-bottom strip coordinates.
#[allow(
    clippy::cast_precision_loss,
    reason = "sample counts and dimensions are far below f32's exact integer range"
)]
fn sample_point(edge: Edge, index: usize, count: usize, width: u32, height: u32) -> [f32; 2] {
    let position = (index as f32 + 0.5) / count as f32;
    match edge {
        Edge::Top => [position * width as f32, 0.0],
        Edge::Right => [width as f32, position * height as f32],
        Edge::Bottom => [position * width as f32, height as f32],
        Edge::Left => [0.0, position * height as f32],
    }
}

/// Fewest cells a worker averages at a time: about 4,000 pixels at 640x360,
/// so a small grid is not split into pieces that cost more to hand out than
/// to compute.
const CELL_CHUNK: usize = 64;
/// Fewest samples a worker projects at a time: four groups of [`GROUP`]. A
/// chunk need not hold whole groups; its last group may be short.
const PROJECTION_CHUNK: usize = 4 * GROUP;
/// Fewest colour focus samples a worker refines at a time.
const FOCUS_CHUNK: usize = 8;

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

impl Geometry {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        reason = "grid coordinates are bounded by u32 dimensions; weights use approximate f32 geometry"
    )]
    fn build(&mut self, key: GeometryKey) {
        let GeometryKey {
            width,
            height,
            density,
            reach,
        } = key;
        let [horizontal, vertical] = density.samples_for_picture(width, height);
        let [columns, rows] = density.grid_size(width, height);
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
        self.columns = columns as usize;
        let radius = height as f32 * reach.as_ratio();
        self.radius = radius;
        self.counts = [horizontal, vertical, horizontal, vertical];
        self.weights.clear();
        let samples = self.counts.iter().sum::<usize>();
        self.weights.reserve_exact(samples * self.cells.len());
        for point in key.sample_points(self.counts) {
            let start = self.weights.len();
            self.weights.extend(self.cells.iter().map(|&cell| {
                let distance_squared = distance_squared(cell, point, radius);
                cell_area(cell) * (broad_tail(distance_squared) + local_lobe(distance_squared))
            }));
            let row = &mut self.weights[start..];
            let total: f64 = row.iter().map(|&w| f64::from(w)).sum();
            for weight in row {
                *weight = (f64::from(*weight) / total) as f32;
            }
        }
        self.step = lattice_step(width, height);
        self.key = key;
    }
}

impl SpatialSampler {
    /// Reduces `view` to edge colours with `config`'s density, reach and
    /// colour focus. Cells, projected samples and colour focus samples are
    /// each independent of the others, so `workers` may compute them in
    /// chunks on its pool; the result is the same, bit for bit, on any number
    /// of threads. `mark` is told as each stage ends.
    pub fn process_with(
        &mut self,
        view: &FrameView,
        config: &Config,
        workers: &Workers,
        mark: &mut impl FnMut(Stage),
    ) -> EdgeColors {
        let (width, height) = (view.viewport_width(), view.viewport_height());
        if width == 0 || height == 0 {
            return EdgeColors::black(width, height, config.sample_density);
        }
        let key = GeometryKey {
            width,
            height,
            density: config.sample_density,
            reach: config.spatial_reach_percent,
        };
        let geometry = &mut self.geometry;
        if geometry.key != key {
            geometry.build(key);
            self.cell_colors
                .resize(geometry.cells.len(), LinearColor::black());
            self.samples
                .resize(geometry.counts.iter().sum(), LinearColor::black());
            self.focus.invalidate();
        }
        let (cells, step) = (&geometry.cells, geometry.step);
        workers.for_each_chunk(&mut self.cell_colors, CELL_CHUNK, |offset, colors| {
            for (color, &[x0, y0, x1, y1]) in colors.iter_mut().zip(&cells[offset..]) {
                *color = view.average_linear_every(x0, y0, x1, y1, step);
            }
        });
        mark(Stage::CellAverages);

        // The samples of all four edges, in strip order, as one run; each
        // sample's weighted mean is independent of the others.
        let (colors, weights) = (&self.cell_colors, &geometry.weights);
        let cell_count = colors.len();
        let samples = &mut self.samples;
        workers.for_each_chunk(samples, PROJECTION_CHUNK, |offset, chunk| {
            let rows = &weights[offset * cell_count..];
            for (out, rows) in chunk.chunks_mut(GROUP).zip(rows.chunks(GROUP * cell_count)) {
                project(colors, rows, out);
            }
        });
        mark(Stage::Projection);

        let focus = config.color_focus_percent.as_ratio();
        if focus > 0.0 {
            self.focus.prepare_frame(geometry, &self.cell_colors);
            workers.for_each_chunk(samples, FOCUS_CHUNK, |offset, chunk| {
                self.focus.refine(offset, focus, chunk);
            });
            mark(Stage::ColorFocus);
        }

        let mut rest = samples.iter().copied();
        let [top, right, bottom, left] = geometry.counts.map(|count| {
            ColorStrip::new(rest.by_ref().take(count).collect()).expect("sample count is nonzero")
        });
        EdgeColors::new(top, right, bottom, left)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::Viewport;
    use prysm_core::{Color, ColorFocusPercent, Frame, PixelFormat};
    use std::ops::{Deref, DerefMut};

    /// A sampler and the settings it is given with each frame.
    pub(super) struct Configured {
        sampler: SpatialSampler,
        config: Config,
    }

    impl Configured {
        pub(super) fn new(density: SampleDensity, reach_percent: u8) -> Self {
            Self {
                sampler: SpatialSampler::default(),
                config: Config {
                    sample_density: density,
                    spatial_reach_percent: SpatialReachPercent(reach_percent),
                    ..Config::default()
                },
            }
        }

        pub(super) fn set_color_focus(&mut self, focus: ColorFocusPercent) {
            self.config.color_focus_percent = focus;
        }

        pub(super) fn set_reach(&mut self, reach_percent: u8) {
            self.config.spatial_reach_percent = SpatialReachPercent(reach_percent);
        }

        /// [`SpatialSampler::process_with`] on the caller's thread alone, unobserved.
        pub(super) fn process(&mut self, view: &FrameView) -> EdgeColors {
            let config = self.config;
            self.sampler
                .process_with(view, &config, &Workers::default(), &mut |_| {})
        }

        /// Builds the geometry of a `width` x `height` picture.
        fn prepare(&mut self, width: u32, height: u32) {
            let key = GeometryKey {
                width,
                height,
                density: self.config.sample_density,
                reach: self.config.spatial_reach_percent,
            };
            self.sampler.geometry.build(key);
        }
    }

    impl Deref for Configured {
        type Target = SpatialSampler;

        fn deref(&self) -> &SpatialSampler {
            &self.sampler
        }
    }

    impl DerefMut for Configured {
        fn deref_mut(&mut self) -> &mut SpatialSampler {
            &mut self.sampler
        }
    }

    pub(super) fn picture(
        width: u32,
        height: u32,
        pixel: impl Fn(u32, u32) -> [u8; 3],
    ) -> FrameView {
        let mut data = Vec::new();
        for y in 0..height {
            for x in 0..width {
                data.extend(pixel(x, y));
            }
        }
        FrameView::new(Frame::new(data, width, height, PixelFormat::RGB24).unwrap())
    }

    pub(super) fn strips(colors: &EdgeColors) -> [&ColorStrip; 4] {
        [&colors.top, &colors.right, &colors.bottom, &colors.left]
    }

    #[test]
    fn yuyv_is_sampled_in_place_and_matches_its_pixels_as_rgb_with_an_offset_crop() {
        use prysm_core::{YuvRange, YuvStandardMatrix};
        let mut sampler = Configured::new(SampleDensity(128), 10);
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
                    let yuyv = prysm_core::yuyv::YuyvView::new(&raw.frame).unwrap();
                    let mut rgb = picture(66, 34, |x, y| yuyv.pixel(x, y).into());
                    rgb.viewport = raw.viewport;
                    assert_eq!(
                        sampler.process(&raw),
                        sampler.process(&rgb),
                        "{range:?} {matrix:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn nv12_is_sampled_in_place_and_matches_its_rgb_conversion_with_an_offset_crop() {
        use prysm_core::{YuvRange, YuvStandardMatrix};
        let mut sampler = Configured::new(SampleDensity(128), 10);
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
        let mut sampler = Configured::new(SampleDensity(700), 15);
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
            .geometry
            .cells
            .iter()
            .map(|&[x0, y0, x1, y1]| view.average_linear(x0, y0, x1, y1))
            .collect();
        assert_eq!(sampler.cell_colors, cells);
        let mut rows = sampler.geometry.weights.chunks_exact(cells.len());
        for (strip, count) in strips(&output).into_iter().zip(sampler.geometry.counts) {
            assert_eq!(strip.len(), count);
            let reference: Vec<_> = rows
                .by_ref()
                .take(strip.len())
                .map(|row| {
                    let sum = |channel: fn(&LinearColor) -> f32| {
                        cells
                            .iter()
                            .zip(row)
                            .map(|(cell, w)| f64::from(channel(cell)) * f64::from(*w))
                            .sum::<f64>() as f32
                    };
                    LinearColor::new(sum(|c| c.r), sum(|c| c.g), sum(|c| c.b))
                })
                .collect();
            assert_eq!(reference.len(), count);
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
        assert!(rows.next().is_none());
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
        let mut sampler = Configured::new(SampleDensity(128), 10);
        let _ = sampler.process(&picture(1280, 720, |_, _| [0; 3]));
        assert_eq!(sampler.geometry.step, 2);
        let mut small = Configured::new(SampleDensity(128), 10);
        let _ = small.process(&picture(640, 360, |_, _| [0; 3]));
        assert_eq!(small.geometry.step, 1);
        assert_eq!(
            sampler.geometry.cells.len(),
            small.geometry.cells.len(),
            "the grid of 640x360"
        );
        let view = picture(1280, 720, |x, y| {
            [
                (x * 7).to_le_bytes()[0],
                (y * 3).to_le_bytes()[0],
                (x ^ y).to_le_bytes()[0],
            ]
        });
        let _ = sampler.process(&view);
        for (&[x0, y0, x1, y1], &actual) in sampler.geometry.cells.iter().zip(&sampler.cell_colors)
        {
            let mut sum = [0.0_f64; 3];
            let mut count = 0.0;
            for y in (y0..y1).step_by(2) {
                for x in (x0..x1).step_by(2) {
                    let pixel = view.average_linear(x, y, x + 1, y + 1);
                    for (sum, value) in sum.iter_mut().zip([pixel.r, pixel.g, pixel.b]) {
                        *sum += f64::from(value);
                    }
                    count += 1.0;
                }
            }
            let expected = sum.map(|channel| (channel / count) as f32);
            assert_eq!([actual.r, actual.g, actual.b], expected);
        }
    }

    #[test]
    fn a_4k_picture_at_the_most_detail_has_the_weights_of_640x360() {
        let density = SampleDensity(usize::from(prysm_ports::Processing::MAX_SAMPLE_DENSITY));
        let mut large = Configured::new(density, 10);
        large.prepare(3840, 2160);
        let mut budget = Configured::new(density, 10);
        budget.prepare(640, 360);
        assert_eq!(large.geometry.counts, budget.geometry.counts);
        assert_eq!(large.geometry.weights.len(), budget.geometry.weights.len());
        // At most one sample per grid cell along each edge: 2 x (128 + 72)
        // samples of 128 x 72 cells, 15 MB of weights.
        assert!(budget.geometry.weights.len() <= 400 * 9216);
        assert_eq!(budget.geometry.counts, [128, 72, 128, 72]);
        let mut sliver = Configured::new(density, 10);
        sliver.prepare(230_400, 1);
        assert_eq!(sliver.geometry.counts, [128, 1, 128, 1]);
        assert_eq!(
            (sliver.geometry.cells.len(), sliver.geometry.columns),
            (128, 128)
        );
    }

    #[test]
    fn a_doubled_picture_lights_exactly_like_the_original() {
        let pixel = |x: u32, y: u32| {
            if x < 200 && y < 180 {
                [255, 64, 0]
            } else if x > 500 {
                [0, 90, 255]
            } else {
                [(x / 3).to_le_bytes()[0], 30, (y / 2).to_le_bytes()[0]]
            }
        };
        let original = Configured::new(SampleDensity(100), 10).process(&picture(640, 360, pixel));
        let doubled =
            Configured::new(SampleDensity(100), 10)
                .process(&picture(1280, 720, |x, y| pixel(x / 2, y / 2)));
        assert_eq!(doubled, original);
    }

    #[test]
    fn small_highlights_on_the_lattice_still_reach_the_edges() {
        let mut sampler = Configured::new(SampleDensity(128), 10);
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
                let mut sampler = Configured::new(SampleDensity(density), 10);
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
        let mut sampler = Configured::new(SampleDensity(250), 10);
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
        let mut sampler = Configured::new(SampleDensity(250), 10);
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
        let mut sampler = Configured::new(SampleDensity(128), 10);
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
        let mut sampler = Configured::new(SampleDensity(128), 10);
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
            let expected =
                Configured::new(SampleDensity(128), 10).process(&picture(width, height, pixel));
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn scaling_the_picture_preserves_influence_in_landscape_and_portrait() {
        for (width, height) in [(64, 36), (36, 64)] {
            let mut sampler = Configured::new(SampleDensity(0), 10);
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
