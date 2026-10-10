use crate::frames::ViewFrame;
use prysm_core::{ColorStrip, Edge, EdgeColors, SampleDensity};

/// Samples edge colors from a raw frame, decoding only the pixels it reads.
///
/// Each color sample integrates its full edge segment in linear light,
/// with quadratic falloff from the screen edge toward the interior.
#[derive(Debug)]
pub struct EdgeSampler {
    sample_density: SampleDensity,
    /// Edge region depth as a fraction of frame height
    edge_depth: f32,
}

impl EdgeSampler {
    pub fn new(sample_density: SampleDensity, edge_depth: f32) -> Self {
        Self {
            sample_density,
            edge_depth,
        }
    }

    /// Edge region depth in pixels for the current viewport.
    ///
    /// Derived from height so the band has uniform thickness on all four
    /// edges, and clamped so opposing regions never overlap.
    fn depth_px(&self, view: &ViewFrame) -> u32 {
        let depth = (view.viewport_height() as f32 * self.edge_depth).round() as u32;
        depth
            .clamp(1, (view.viewport_height() / 2).max(1))
            .min((view.viewport_width() / 2).max(1))
    }

    fn sample_edge(&self, view: &ViewFrame, edge: Edge) -> ColorStrip {
        let width = view.viewport_width();
        let height = view.viewport_height();
        let depth = self.depth_px(view);

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

        let mut samples = Vec::with_capacity(sample_count);

        for i in 0..sample_count {
            let segment_start = (i as u64 * u64::from(edge_length) / sample_count as u64) as u32;
            let segment_end =
                ((i + 1) as u64 * u64::from(edge_length) / sample_count as u64) as u32;

            // Define sampling region (viewport-relative)
            let (x_start, y_start, x_end, y_end) = match edge {
                Edge::Top => (segment_start, 0, segment_end, depth),
                Edge::Bottom => (segment_start, height - depth, segment_end, height),
                Edge::Left => (0, segment_start, depth, segment_end),
                Edge::Right => (width - depth, segment_start, width, segment_end),
            };

            let color = view.average_edge_linear(x_start, y_start, x_end, y_end, edge);
            samples.push(color);
        }

        ColorStrip::new(samples)
    }

    pub fn process(&self, input: &ViewFrame) -> EdgeColors {
        let top = self.sample_edge(input, Edge::Top);
        let right = self.sample_edge(input, Edge::Right);
        let bottom = self.sample_edge(input, Edge::Bottom);
        let left = self.sample_edge(input, Edge::Left);

        EdgeColors::new(top, right, bottom, left)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::yuyv_frame_from_luma;
    use prysm_core::{Color, LinearColor};

    fn sampler() -> EdgeSampler {
        let config = prysm_core::Config::default();
        EdgeSampler::new(
            config.sample_density,
            f32::from(config.edge_depth_percent) / 100.0,
        )
    }

    #[test]
    fn uniform_frame_yields_uniform_edge_colors() {
        let frame = yuyv_frame_from_luma(640, 360, |_, _| 128);
        let edge_colors = sampler().process(&ViewFrame::new(frame));

        let expected = LinearColor::from_srgb(Color::new(128, 128, 128));
        for strip in [
            &edge_colors.top,
            &edge_colors.right,
            &edge_colors.bottom,
            &edge_colors.left,
        ] {
            let color = strip.sample_at(0.5);
            assert!(
                (color.r - expected.r).abs() < 0.01,
                "expected uniform gray, got {color:?}"
            );
        }
        // Density 60/1000px: 640px edge -> 38 samples, 360px edge -> 21
        assert_eq!(edge_colors.top.len(), 38);
        assert_eq!(edge_colors.left.len(), 21);
    }

    #[test]
    fn top_and_bottom_edges_differ() {
        // Top half white, bottom half black
        let frame = yuyv_frame_from_luma(640, 360, |_, y| if y < 180 { 255 } else { 0 });
        let edge_colors = sampler().process(&ViewFrame::new(frame));

        assert!(edge_colors.top.sample_at(0.5).r > 0.99);
        assert!(edge_colors.bottom.sample_at(0.5).r < 0.01);
    }

    #[test]
    fn oversampling_a_white_frame_does_not_create_black_samples() {
        let frame = yuyv_frame_from_luma(8, 8, |_, _| 255);
        let output = EdgeSampler::new(SampleDensity(2000), 0.03).process(&ViewFrame::new(frame));
        for edge in [&output.top, &output.bottom, &output.left, &output.right] {
            for color in edge.resample(edge.len()) {
                assert_eq!(color, LinearColor::new(1.0, 1.0, 1.0));
            }
            assert_eq!(edge.len(), 8);
        }
    }

    #[test]
    fn depth_clamps_on_tiny_viewports() {
        // 8x8 frame: the requested depth rounds to one pixel.
        let frame = yuyv_frame_from_luma(8, 8, |_, _| 200);
        let edge_colors = sampler().process(&ViewFrame::new(frame));
        assert!(edge_colors.top.sample_at(0.5).r > 0.0);
    }
}
