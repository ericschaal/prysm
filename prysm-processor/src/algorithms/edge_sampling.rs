use crate::algorithm::Algorithm;
use crate::pixel_reader::PixelReader;
use crate::pixel_readers::{Rgb24Reader, YuyvReader};
use prysm_capture::{Frame, PixelFormat};
use prysm_core::{Color, ColorSpectrum, Edge, EdgeSpectra, SampleDensity};
use std::ops::Range;

/// Edge sampling algorithm - analyzes edge regions and extracts color spectra
///
/// Divides each screen edge into segments and samples the average color
/// in each segment from a configurable depth inward from the edge.
///
/// Configuration is stored in the algorithm instance for zero per-frame overhead.
#[derive(Debug, Clone, Copy)]
pub struct EdgeSamplingAlgorithm {
    /// Sample step for region averaging (e.g., 4 = sample every 4th pixel)
    sample_step: usize,
    /// Sample density per 1000 pixels of edge length
    sample_density: SampleDensity,
    /// Depth of edge sampling in pixels from the screen edge
    edge_depth_px: u32,
}

impl Default for EdgeSamplingAlgorithm {
    fn default() -> Self {
        Self {
            sample_step: 1,
            edge_depth_px: 1,
            sample_density: SampleDensity(1000),
        }
    }
}

impl Algorithm for EdgeSamplingAlgorithm {
    fn process(&self, frame: &Frame) -> EdgeSpectra {
        match frame.format {
            PixelFormat::RGB24 => self.process_with_reader(frame, &Rgb24Reader),
            PixelFormat::YUYV => self.process_with_reader(frame, &YuyvReader),
            PixelFormat::MJPEG | PixelFormat::BGR24 => {
                tracing::error!("{} format not yet supported", frame.format);
                EdgeSpectra::black(
                    frame.width as usize,
                    frame.height as usize,
                    self.sample_density,
                )
            }
        }
    }
}

impl EdgeSamplingAlgorithm {
    #[must_use]
    pub fn new(sample_step: usize, sample_density: SampleDensity, edge_depth_px: u32) -> Self {
        Self {
            sample_step,
            sample_density,
            edge_depth_px,
        }
    }

    /// Process frame using a specific pixel reader (generic over format)
    fn process_with_reader<R: PixelReader>(&self, frame: &Frame, reader: &R) -> EdgeSpectra {
        let width = frame.width;
        let height = frame.height;

        let top_spectrum =
            self.extract_edge_spectrum(&frame.data, width, height, Edge::Top, reader);
        let right_spectrum =
            self.extract_edge_spectrum(&frame.data, width, height, Edge::Right, reader);
        let bottom_spectrum =
            self.extract_edge_spectrum(&frame.data, width, height, Edge::Bottom, reader);
        let left_spectrum =
            self.extract_edge_spectrum(&frame.data, width, height, Edge::Left, reader);

        EdgeSpectra::new(top_spectrum, right_spectrum, bottom_spectrum, left_spectrum)
    }

    /// Extract color spectrum from a specific edge (generic over pixel format)
    fn extract_edge_spectrum<R: PixelReader>(
        &self,
        data: &[u8],
        width: u32,
        height: u32,
        edge: Edge,
        reader: &R,
    ) -> ColorSpectrum {
        // Calculate sample count based on edge length
        let (edge_length, sample_count) = match edge {
            Edge::Top | Edge::Bottom => {
                let length = width;
                let samples = self.sample_density.samples_for_length(length as usize);
                (length, samples)
            }
            Edge::Left | Edge::Right => {
                let length = height;
                let samples = self.sample_density.samples_for_length(length as usize);
                (length, samples)
            }
        };

        // Use fixed edge depth in pixels (uniform for all edges)
        let edge_depth = self.edge_depth_px;

        let segment_length = edge_length as f32 / sample_count as f32;

        let mut samples = Vec::with_capacity(sample_count);

        for i in 0..sample_count {
            let segment_start = (i as f32 * segment_length) as u32;
            let segment_end = ((i + 1) as f32 * segment_length).min(edge_length as f32) as u32;

            // Define the sampling region for this segment
            let (x_start, y_start, x_end, y_end) = match edge {
                Edge::Top => (segment_start, 0, segment_end, edge_depth),
                Edge::Bottom => (segment_start, height - edge_depth, segment_end, height),
                Edge::Left => (0, segment_start, edge_depth, segment_end),
                Edge::Right => (width - edge_depth, segment_start, width, segment_end),
            };

            let x_range = x_start..x_end;
            let y_range = y_start..y_end;

            let color = self.average_color_in_region(data, width, height, x_range, y_range, reader);
            samples.push(color);
        }

        ColorSpectrum::new(samples)
    }

    /// Calculate average color in a rectangular region
    fn average_color_in_region<R: PixelReader>(
        &self,
        data: &[u8],
        width: u32,
        height: u32,
        x_range: Range<u32>,
        y_range: Range<u32>,
        reader: &R,
    ) -> Color {
        let mut r_sum: u64 = 0;
        let mut g_sum: u64 = 0;
        let mut b_sum: u64 = 0;
        let mut count: u64 = 0;

        let color_region =
            reader.read_region(data, width, height, x_range.clone(), y_range.clone());
        let region_width = x_range.len();
        for y_offset in (0..y_range.len()).step_by(self.sample_step) {
            for x_offset in (0..x_range.len()).step_by(self.sample_step) {
                let idx = y_offset * region_width + x_offset;
                if idx < color_region.len() {
                    let color = color_region[idx];
                    r_sum += u64::from(color.r);
                    g_sum += u64::from(color.g);
                    b_sum += u64::from(color.b);
                    count += 1;
                }
            }
        }

        if count == 0 {
            return Color::black();
        }

        Color::new(
            (r_sum / count) as u8,
            (g_sum / count) as u8,
            (b_sum / count) as u8,
        )
    }
}
