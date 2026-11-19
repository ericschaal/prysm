use crate::Stage;
use crate::frames::FrameView;
use crate::workers::Workers;
use color_focus::ColorFocus;
use prysm_core::{ColorFocusPercent, ColorStrip, Edge, EdgeColors, LinearColor, SampleDensity};
use std::ops::Range;

mod color_focus;

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

/// Projects the whole picture onto the perimeter using a smooth distance
/// falloff, then optionally lets each sample keep the colour next to it
/// ([`ColorFocus`]).
#[derive(Debug)]
pub struct SpatialSampler {
    sample_density: SampleDensity,
    reach_ratio: f32,
    color_focus: ColorFocusPercent,
    dimensions: Option<(u32, u32)>,
    step: u32,
    // Cell rectangles `[x0, y0, x1, y1]`, row by row, `columns` to a row.
    cells: Vec<[u32; 4]>,
    columns: usize,
    cell_colors: Vec<LinearColor>,
    // The kernel radius in pixels, and where each output sample sits on the
    // picture's edge, in strip order.
    radius: f32,
    sample_points: [Vec<[f32; 2]>; 4],
    // One row of normalized cell weights per output sample, in strip order.
    sample_weights: [Vec<f32>; 4],
    focus: ColorFocus,
}

/// The sampler's geometry, as the colour focus pass reads it.
struct Geometry<'a> {
    width: u32,
    height: u32,
    cells: &'a [[u32; 4]],
    columns: usize,
    radius: f32,
    points: &'a [Vec<[f32; 2]>; 4],
    weights: &'a [Vec<f32>; 4],
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

/// The parts of a chunk of the run of all edges' samples (edges in strip
/// order, `counts[edge]` samples each) that starts at sample `offset` and
/// holds `len`: for each edge it overlaps, the edge, the part's samples
/// within the edge, and within the chunk.
fn edge_parts(
    counts: [usize; 4],
    offset: usize,
    len: usize,
) -> impl Iterator<Item = (usize, Range<usize>, Range<usize>)> {
    let ends = counts.into_iter().scan(0, |end, count| {
        *end += count;
        Some(*end)
    });
    ends.zip(counts)
        .enumerate()
        .filter_map(move |(edge, (end, count))| {
            let start = end - count;
            let (first, last) = (offset.max(start), (offset + len).min(end));
            (first < last).then(|| {
                (
                    edge,
                    first - start..last - start,
                    first - offset..last - offset,
                )
            })
        })
}

impl SpatialSampler {
    pub fn new(sample_density: SampleDensity, reach_ratio: f32) -> Self {
        Self {
            sample_density,
            reach_ratio,
            color_focus: ColorFocusPercent(0),
            dimensions: None,
            step: 1,
            cells: Vec::new(),
            columns: 0,
            cell_colors: Vec::new(),
            radius: 0.0,
            sample_points: std::array::from_fn(|_| Vec::new()),
            sample_weights: std::array::from_fn(|_| Vec::new()),
            focus: ColorFocus::default(),
        }
    }

    /// How much each sample keeps the colour next to it; 0 is the plain
    /// weighted mean, exactly. Geometry is kept.
    pub fn set_color_focus(&mut self, color_focus: ColorFocusPercent) {
        self.color_focus = color_focus;
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
        self.columns = columns as usize;
        let radius = height as f32 * self.reach_ratio;
        self.radius = radius;
        let edges = self.sample_weights.iter_mut().zip(&mut self.sample_points);
        for ((weights, points), (edge, count)) in edges.zip([
            (Edge::Top, horizontal),
            (Edge::Right, vertical),
            (Edge::Bottom, horizontal),
            (Edge::Left, vertical),
        ]) {
            weights.clear();
            points.clear();
            points.extend((0..count).map(|i| sample_point(edge, i, count, width, height)));
            for &point in points.iter() {
                let start = weights.len();
                weights.extend(self.cells.iter().map(|&cell| {
                    let distance_squared = distance_squared(cell, point, radius);
                    cell_area(cell) * (broad_tail(distance_squared) + local_lobe(distance_squared))
                }));
                // Geometry-only normalization keeps black and small highlights proportional.
                let total: f64 = weights[start..].iter().map(|&w| f64::from(w)).sum();
                for weight in &mut weights[start..] {
                    *weight = (f64::from(*weight) / total) as f32;
                }
            }
        }
        self.focus.invalidate();
        self.step = lattice_step(width, height);
        self.dimensions = Some((width, height));
    }

    /// [`Self::process_with`] on the caller's thread alone, unobserved.
    #[cfg(test)]
    pub fn process(&mut self, view: &FrameView) -> EdgeColors {
        self.process_with(view, &Workers::default(), &mut |_| {})
    }

    /// Reduces `view` to edge colours. Cells, projected samples and colour
    /// focus samples are each independent of the others, so `workers` may compute them in chunks on its pool; the
    /// result is the same, bit for bit, on any number of threads. `mark` is
    /// told as each stage ends.
    pub fn process_with(
        &mut self,
        view: &FrameView,
        workers: &Workers,
        mark: &mut impl FnMut(Stage),
    ) -> EdgeColors {
        let (width, height) = (view.viewport_width(), view.viewport_height());
        if width == 0 || height == 0 {
            return EdgeColors::black(width as usize, height as usize, self.sample_density);
        }
        if self.dimensions != Some((width, height)) {
            self.prepare_geometry(width, height);
        }
        let (cells, step) = (&self.cells, self.step);
        workers.for_each_chunk(&mut self.cell_colors, CELL_CHUNK, |offset, colors| {
            for (color, &[x0, y0, x1, y1]) in colors.iter_mut().zip(&cells[offset..]) {
                *color = view.average_linear_every(x0, y0, x1, y1, step);
            }
        });
        mark(Stage::CellAverages);

        // The samples of all four edges, in strip order, as one run; each
        // sample's weighted mean is independent of the others.
        let (colors, weights) = (&self.cell_colors, &self.sample_weights);
        let cell_count = colors.len();
        let counts = weights.each_ref().map(|w| w.len() / cell_count);
        let mut samples = vec![LinearColor::black(); counts.iter().sum()];
        workers.for_each_chunk(&mut samples, PROJECTION_CHUNK, |offset, chunk| {
            for (edge, rows, out) in edge_parts(counts, offset, chunk.len()) {
                let rows = &weights[edge][rows.start * cell_count..rows.end * cell_count];
                let groups = chunk[out]
                    .chunks_mut(GROUP)
                    .zip(rows.chunks(GROUP * cell_count));
                groups.for_each(|(out, rows)| project(colors, rows, out));
            }
        });
        mark(Stage::Projection);

        let focus = self.color_focus.as_ratio();
        if focus > 0.0 {
            let geometry = Geometry {
                width,
                height,
                cells: &self.cells,
                columns: self.columns,
                radius: self.radius,
                points: &self.sample_points,
                weights: &self.sample_weights,
            };
            self.focus.prepare_frame(&geometry, &self.cell_colors);
            let focus_pass = &self.focus;
            workers.for_each_chunk(&mut samples, FOCUS_CHUNK, |offset, chunk| {
                for (edge, rows, out) in edge_parts(counts, offset, chunk.len()) {
                    focus_pass.refine(edge, rows.start, focus, &mut chunk[out]);
                }
            });
            mark(Stage::ColorFocus);
        }

        let mut rest = samples.into_iter();
        let [top, right, bottom, left] = counts.map(|count| {
            ColorStrip::new(rest.by_ref().take(count).collect()).expect("sample count is nonzero")
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
            .map(|&[x0, y0, x1, y1]| view.average_linear(x0, y0, x1, y1))
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
    fn edge_parts_split_a_chunk_of_the_run_at_edge_boundaries() {
        let parts = |offset, len| edge_parts([3, 2, 0, 4], offset, len).collect::<Vec<_>>();
        assert_eq!(
            parts(0, 9),
            [(0, 0..3, 0..3), (1, 0..2, 3..5), (3, 0..4, 5..9)]
        );
        assert_eq!(
            parts(2, 4),
            [(0, 2..3, 0..1), (1, 0..2, 1..3), (3, 0..1, 3..4)]
        );
        assert_eq!(parts(6, 2), [(3, 1..3, 0..2)]);
        assert!(parts(9, 0).is_empty());
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
                    let pixel = view.average_linear(x, y, x + 1, y + 1);
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
