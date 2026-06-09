use crate::frames::ViewFrame;
use crate::pipeline::Node;
use prysm_core::{Edge, EdgeSpectra, SampleDensity, Spectrum};

/// Samples edge colors from a raw frame, decoding only the pixels it reads.
///
/// Each LED sample is the linear-light average of its full edge segment,
/// integrating every pixel in the region rather than point-sampling a
/// sparse grid.
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

    fn extract_edge_spectrum(&self, view: &ViewFrame, edge: Edge) -> Spectrum {
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

        let segment_length = edge_length as f32 / sample_count as f32;
        let mut samples = Vec::with_capacity(sample_count);

        for i in 0..sample_count {
            let segment_start = (i as f32 * segment_length) as u32;
            let segment_end = ((i + 1) as f32 * segment_length).min(edge_length as f32) as u32;

            // Define sampling region (viewport-relative)
            let (x_start, y_start, x_end, y_end) = match edge {
                Edge::Top => (segment_start, 0, segment_end, depth),
                Edge::Bottom => (segment_start, height - depth, segment_end, height),
                Edge::Left => (0, segment_start, depth, segment_end),
                Edge::Right => (width - depth, segment_start, width, segment_end),
            };

            let color = view.average_linear(x_start, y_start, x_end, y_end);
            samples.push(color);
        }

        Spectrum::new(samples)
    }
}

impl Node<ViewFrame, EdgeSpectra> for EdgeSampler {
    fn process(&mut self, input: ViewFrame) -> EdgeSpectra {
        let top = self.extract_edge_spectrum(&input, Edge::Top);
        let right = self.extract_edge_spectrum(&input, Edge::Right);
        let bottom = self.extract_edge_spectrum(&input, Edge::Bottom);
        let left = self.extract_edge_spectrum(&input, Edge::Left);

        EdgeSpectra::new(top, right, bottom, left)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::yuyv_frame_from_luma;
    use prysm_core::{Color, LinearColor};

    fn sampler() -> EdgeSampler {
        let config = prysm_core::Config::default();
        EdgeSampler::new(config.sample_density, config.edge_depth)
    }

    #[test]
    fn uniform_frame_yields_uniform_spectra() {
        let frame = yuyv_frame_from_luma(640, 360, |_, _| 128);
        let spectra = sampler().process(ViewFrame::new(frame));

        let expected = LinearColor::from_srgb(Color::new(128, 128, 128));
        for spectrum in [&spectra.top, &spectra.right, &spectra.bottom, &spectra.left] {
            let color = spectrum.sample_at(0.5);
            assert!(
                (color.r - expected.r).abs() < 0.01,
                "expected uniform gray, got {color:?}"
            );
        }
        // Density 30/1000px: 640px edge -> 19 samples, 360px edge -> 10
        assert_eq!(spectra.top.len(), 19);
        assert_eq!(spectra.left.len(), 10);
    }

    #[test]
    fn top_and_bottom_edges_differ() {
        // Top half white, bottom half black
        let frame = yuyv_frame_from_luma(640, 360, |_, y| if y < 180 { 255 } else { 0 });
        let spectra = sampler().process(ViewFrame::new(frame));

        assert!(spectra.top.sample_at(0.5).r > 0.99);
        assert!(spectra.bottom.sample_at(0.5).r < 0.01);
    }

    #[test]
    fn depth_clamps_on_tiny_viewports() {
        // 8x8 frame: requested depth (0.09 * 8 ~ 1px) must stay within bounds
        let frame = yuyv_frame_from_luma(8, 8, |_, _| 200);
        let spectra = sampler().process(ViewFrame::new(frame));
        assert!(spectra.top.sample_at(0.5).r > 0.0);
    }
}
