use crate::algorithm::Algorithm;
use crate::pixel_reader::PixelReader;
use crate::pixel_readers::{Rgb24Reader, YuyvReader};
use prysm_capture::{Frame, PixelFormat};
use prysm_core::{Color, ColorSpectrum, Config, Edge, EdgeSpectrums};

/// Edge sampling algorithm - analyzes edge regions and extracts color spectrums
///
/// Divides each screen edge into segments and samples the average color
/// in each segment from a configurable depth inward from the edge.
#[derive(Debug, Clone, Copy, Default)]
pub struct EdgeSamplingAlgorithm;

impl Algorithm for EdgeSamplingAlgorithm {
    fn process(&self, frame: &Frame, config: &Config) -> EdgeSpectrums {
        match frame.format {
            PixelFormat::RGB24 => self.process_with_reader(frame, config, &Rgb24Reader),
            PixelFormat::YUYV => self.process_with_reader(frame, config, &YuyvReader),
            PixelFormat::MJPEG | PixelFormat::BGR24 => {
                tracing::error!("{} format not yet supported", frame.format);
                EdgeSpectrums::black(
                    frame.width as usize,
                    frame.height as usize,
                    config.samples_per_1000px,
                )
            }
        }
    }
}

impl EdgeSamplingAlgorithm {
    /// Process frame using a specific pixel reader (generic over format)
    fn process_with_reader<R: PixelReader>(
        &self,
        frame: &Frame,
        config: &Config,
        reader: &R,
    ) -> EdgeSpectrums {
        let width = frame.width;
        let height = frame.height;

        // Extract spectrum for each edge using the pixel reader
        let top_spectrum = self.extract_edge_spectrum(&frame.data, width, height, Edge::Top, config, reader);
        let right_spectrum = self.extract_edge_spectrum(&frame.data, width, height, Edge::Right, config, reader);
        let bottom_spectrum = self.extract_edge_spectrum(&frame.data, width, height, Edge::Bottom, config, reader);
        let left_spectrum = self.extract_edge_spectrum(&frame.data, width, height, Edge::Left, config, reader);

        EdgeSpectrums::new(top_spectrum, right_spectrum, bottom_spectrum, left_spectrum)
    }

    /// Extract color spectrum from a specific edge (generic over pixel format)
    ///
    /// This consolidates the previously duplicated RGB24 and YUYV implementations
    /// into a single generic method using the PixelReader trait.
    fn extract_edge_spectrum<R: PixelReader>(
        &self,
        data: &[u8],
        width: u32,
        height: u32,
        edge: Edge,
        config: &Config,
        reader: &R,
    ) -> ColorSpectrum {
        // Calculate sample count based on edge length
        let (edge_length, sample_count) = match edge {
            Edge::Top | Edge::Bottom => {
                let length = width;
                let samples = ((length as f32 / 1000.0) * config.samples_per_1000px as f32)
                    .max(1.0) as usize;
                (length, samples)
            }
            Edge::Left | Edge::Right => {
                let length = height;
                let samples = ((length as f32 / 1000.0) * config.samples_per_1000px as f32)
                    .max(1.0) as usize;
                (length, samples)
            }
        };

        // Use fixed edge depth in pixels (uniform for all edges)
        let edge_depth = config.edge_depth_px;

        let segment_length = edge_length as f32 / sample_count as f32;

        // Extract color samples along the edge
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

            let color = Self::average_color_in_region(data, width, x_start, y_start, x_end, y_end, reader);
            samples.push(color);
        }

        ColorSpectrum::new(samples)
    }

    /// Calculate average color in a rectangular region (generic over pixel format)
    ///
    /// This consolidates the previously separate RGB24 and YUYV averaging methods
    /// into a single generic implementation using the PixelReader trait.
    fn average_color_in_region<R: PixelReader>(
        data: &[u8],
        width: u32,
        x_start: u32,
        y_start: u32,
        x_end: u32,
        y_end: u32,
        reader: &R,
    ) -> Color {
        let mut r_sum: u64 = 0;
        let mut g_sum: u64 = 0;
        let mut b_sum: u64 = 0;
        let mut count: u64 = 0;

        // Sample every Nth pixel for performance
        let sample_step = 4;

        for y in (y_start..y_end).step_by(sample_step) {
            for x in (x_start..x_end).step_by(sample_step) {
                let color = reader.read_pixel(data, x, y, width);
                r_sum += color.r as u64;
                g_sum += color.g as u64;
                b_sum += color.b as u64;
                count += 1;
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
