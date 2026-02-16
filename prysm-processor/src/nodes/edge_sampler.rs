use crate::frames::ColorFrame;
use crate::pipeline::Node;
use prysm_core::{Edge, EdgeSpectra, LinearColor, SampleDensity, Spectrum};

/// Samples edge colors from ColorFrame
#[derive(Debug)]
pub struct EdgeSampler {
    sample_step: usize,
    sample_density: SampleDensity,
    edge_depth_px: u32,
}

impl EdgeSampler {
    pub fn new(sample_step: usize, sample_density: SampleDensity, edge_depth_px: u32) -> Self {
        Self {
            sample_step,
            sample_density,
            edge_depth_px,
        }
    }

    fn extract_edge_spectrum(&self, frame: &ColorFrame, edge: Edge) -> Spectrum {
        let width = frame.viewport_width();
        let height = frame.viewport_height();

        // Calculate sample count based on edge length
        let (edge_length, sample_count) = match edge {
            Edge::Top | Edge::Bottom => {
                let samples = self.sample_density.samples_for_length(width as usize);
                (width, samples)
            }
            Edge::Left | Edge::Right => {
                let samples = self.sample_density.samples_for_length(height as usize);
                (height, samples)
            }
        };

        let segment_length = edge_length as f32 / sample_count as f32;
        let mut samples = Vec::with_capacity(sample_count);

        for i in 0..sample_count {
            let segment_start = (i as f32 * segment_length) as u32;
            let segment_end = ((i + 1) as f32 * segment_length).min(edge_length as f32) as u32;

            // Define sampling region (viewport-relative)
            let (x_start, y_start, x_end, y_end) = match edge {
                Edge::Top => (segment_start, 0, segment_end, self.edge_depth_px),
                Edge::Bottom => (
                    segment_start,
                    height - self.edge_depth_px,
                    segment_end,
                    height,
                ),
                Edge::Left => (0, segment_start, self.edge_depth_px, segment_end),
                Edge::Right => (width - self.edge_depth_px, segment_start, width, segment_end),
            };

            let color = self.average_color_in_region(frame, x_start, y_start, x_end, y_end);
            samples.push(color);
        }

        Spectrum::new(samples)
    }

    fn average_color_in_region(
        &self,
        frame: &ColorFrame,
        x_start: u32,
        y_start: u32,
        x_end: u32,
        y_end: u32,
    ) -> LinearColor {
        let mut r_sum: f32 = 0.0;
        let mut g_sum: f32 = 0.0;
        let mut b_sum: f32 = 0.0;
        let mut count: u32 = 0;

        for y in (y_start..y_end).step_by(self.sample_step) {
            for x in (x_start..x_end).step_by(self.sample_step) {
                if let Some(color) = frame.get_pixel(x, y) {
                    let linear = LinearColor::from_srgb(color);
                    r_sum += linear.r;
                    g_sum += linear.g;
                    b_sum += linear.b;
                    count += 1;
                }
            }
        }

        if count == 0 {
            return LinearColor::black();
        }

        let inv_count = 1.0 / count as f32;
        LinearColor::new(r_sum * inv_count, g_sum * inv_count, b_sum * inv_count)
    }
}

impl Node<ColorFrame, EdgeSpectra> for EdgeSampler {
    fn process(&mut self, input: ColorFrame) -> EdgeSpectra {
        let top = self.extract_edge_spectrum(&input, Edge::Top);
        let right = self.extract_edge_spectrum(&input, Edge::Right);
        let bottom = self.extract_edge_spectrum(&input, Edge::Bottom);
        let left = self.extract_edge_spectrum(&input, Edge::Left);

        EdgeSpectra::new(top, right, bottom, left)
    }
}
