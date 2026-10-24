use crate::frames::ViewFrame;
use prysm_core::{ColorStrip, Edge, EdgeColors, LinearColor, SampleDensity};

/// Projects the whole picture onto the perimeter using a smooth distance falloff.
#[derive(Debug)]
pub struct SpatialSampler {
    sample_density: SampleDensity,
    reach: f32,
    dimensions: Option<(u32, u32)>,
    cells: Vec<[u32; 4]>,
    colors: Vec<LinearColor>,
    // One row of normalized cell weights per output sample, in strip order.
    weights: [Vec<f32>; 4],
}

impl SpatialSampler {
    pub fn new(sample_density: SampleDensity, reach: f32) -> Self {
        Self {
            sample_density,
            reach,
            dimensions: None,
            cells: Vec::new(),
            colors: Vec::new(),
            weights: std::array::from_fn(|_| Vec::new()),
        }
    }

    fn prepare(&mut self, width: u32, height: u32) {
        let horizontal = self.sample_density.samples_for_length(width as usize);
        let vertical = self.sample_density.samples_for_length(height as usize);
        // shortcut: cap the grid at 128 cells per axis; raise only if footage shows lost detail.
        let longest = horizontal.max(vertical).clamp(64, 128) as u32;
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
        self.colors.resize(self.cells.len(), LinearColor::black());
        let radius = height as f32 * self.reach;
        for (weights, (edge, count)) in self.weights.iter_mut().zip([
            (Edge::Top, horizontal),
            (Edge::Right, vertical),
            (Edge::Bottom, horizontal),
            (Edge::Left, vertical),
        ]) {
            weights.clear();
            for i in 0..count {
                let position = (i as f32 + 0.5) / count as f32;
                // Preserve the existing left-to-right / top-to-bottom strip coordinates.
                let (px, py) = match edge {
                    Edge::Top => (position * width as f32, 0.0),
                    Edge::Right => (width as f32, position * height as f32),
                    Edge::Bottom => (position * width as f32, height as f32),
                    Edge::Left => (0.0, position * height as f32),
                };
                let start = weights.len();
                for &[x0, y0, x1, y1] in &self.cells {
                    let dx = ((x0 as f32).midpoint(x1 as f32) - px) / radius;
                    let dy = ((y0 as f32).midpoint(y1 as f32) - py) / radius;
                    let area = (x1 - x0) as f32 * (y1 - y0) as f32;
                    weights.push(area / (1.0 + dx * dx + dy * dy));
                }
                // Geometry-only normalization keeps black and small highlights proportional.
                let total: f64 = weights[start..].iter().map(|&w| f64::from(w)).sum();
                for weight in &mut weights[start..] {
                    *weight = (f64::from(*weight) / total) as f32;
                }
            }
        }
        self.dimensions = Some((width, height));
    }

    pub fn process(&mut self, view: &ViewFrame) -> EdgeColors {
        let (width, height) = (view.viewport_width(), view.viewport_height());
        if width == 0 || height == 0 {
            return EdgeColors::black(width as usize, height as usize, self.sample_density);
        }
        if self.dimensions != Some((width, height)) {
            self.prepare(width, height);
        }
        for (color, &[x0, y0, x1, y1]) in self.colors.iter_mut().zip(&self.cells) {
            *color = view.average_linear(x0, y0, x1, y1);
        }
        let [top, right, bottom, left] = self.weights.each_ref().map(|weights| {
            ColorStrip::new(
                weights
                    .chunks_exact(self.colors.len())
                    .map(|row| {
                        let mut sum = [0.0f64; 3];
                        for (color, &weight) in self.colors.iter().zip(row) {
                            for (channel, value) in sum.iter_mut().zip([color.r, color.g, color.b])
                            {
                                *channel += f64::from(value) * f64::from(weight);
                            }
                        }
                        LinearColor::new(sum[0] as f32, sum[1] as f32, sum[2] as f32)
                    })
                    .collect(),
            )
        });
        EdgeColors::new(top, right, bottom, left)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::Viewport;
    use prysm_capture::{Frame, PixelFormat};
    use prysm_core::Color;

    fn picture(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> ViewFrame {
        let mut data = Vec::new();
        for y in 0..height {
            for x in 0..width {
                data.extend(pixel(x, y));
            }
        }
        ViewFrame::new(Frame::new(data, width, height, PixelFormat::RGB24))
    }

    fn strips(colors: &EdgeColors) -> [&ColorStrip; 4] {
        [&colors.top, &colors.right, &colors.bottom, &colors.left]
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
                    assert!(
                        color.r < 0.1,
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
                assert!(values.windows(2).all(|p| (p[0].r - p[1].r).abs() < 0.2));
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
