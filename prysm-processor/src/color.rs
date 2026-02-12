use image::{ImageBuffer, Rgb};
use prysm_capture::Frame;
use prysm_core::{Config};
use prysm_render::{Color, ColorSpectrum, Edge, EdgeSpectrums};

#[derive(Debug)]
pub struct ColorProcessor {
    config: Config,
    previous_spectrums: Option<EdgeSpectrums>,
}

impl Default for ColorProcessor {
    fn default() -> Self {
        Self {
            config: Config::default(),
            previous_spectrums: None,
        }
    }
}

impl ColorProcessor {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            previous_spectrums: None,
        }
    }

    /// Process a frame and extract color spectrums for each edge
    pub fn process_frame(&mut self, frame: &Frame) -> EdgeSpectrums {
        // Convert frame data to image buffer for easier processing
        let img = ImageBuffer::<Rgb<u8>, _>::from_raw(
            frame.width,
            frame.height,
            frame.data.clone(),
        );

        if img.is_none() {
            tracing::error!("Failed to create image buffer from frame");
            // Return black spectrums on error
            return EdgeSpectrums::black(
                frame.width as usize,
                frame.height as usize,
                self.config.samples_per_1000px,
            );
        }

        let img = img.unwrap();

        // Extract spectrum for each edge
        let top_spectrum = self.extract_edge_spectrum(&img, Edge::Top);
        let right_spectrum = self.extract_edge_spectrum(&img, Edge::Right);
        let bottom_spectrum = self.extract_edge_spectrum(&img, Edge::Bottom);
        let left_spectrum = self.extract_edge_spectrum(&img, Edge::Left);

        let current_spectrums = EdgeSpectrums::new(
            top_spectrum,
            right_spectrum,
            bottom_spectrum,
            left_spectrum,
        );

        // Apply temporal smoothing
        let smoothed_spectrums = if let Some(ref prev) = self.previous_spectrums {
            prev.blend(&current_spectrums, 1.0 - self.config.smoothing)
        } else {
            current_spectrums.clone()
        };

        self.previous_spectrums = Some(smoothed_spectrums.clone());
        smoothed_spectrums
    }

    /// Extract color spectrum from a specific edge
    fn extract_edge_spectrum(&self, img: &ImageBuffer<Rgb<u8>, Vec<u8>>, edge: Edge) -> ColorSpectrum {
        let (width, height) = img.dimensions();

        // Calculate sample count based on edge length
        let (edge_length, sample_count) = match edge {
            Edge::Top | Edge::Bottom => {
                let length = width;
                let samples = ((length as f32 / 1000.0) * self.config.samples_per_1000px as f32).max(1.0) as usize;
                (length, samples)
            }
            Edge::Left | Edge::Right => {
                let length = height;
                let samples = ((length as f32 / 1000.0) * self.config.samples_per_1000px as f32).max(1.0) as usize;
                (length, samples)
            }
        };

        // Use fixed edge depth in pixels (uniform for all edges)
        let edge_depth = self.config.edge_depth_px;

        let segment_length = edge_length as f32 / sample_count as f32;

        // Extract color samples along the edge
        let mut samples = Vec::with_capacity(sample_count);

        for i in 0..sample_count {
            let segment_start = (i as f32 * segment_length) as u32;
            let segment_end = ((i + 1) as f32 * segment_length).min(edge_length as f32) as u32;

            // Define the sampling region for this segment
            // All edges sample in consistent directions that match rendering:
            // - Horizontal edges: left to right
            // - Vertical edges: top to bottom
            let (x_start, y_start, x_end, y_end) = match edge {
                Edge::Top => {
                    // Sample from top edge, left to right
                    (segment_start, 0, segment_end, edge_depth)
                }
                Edge::Bottom => {
                    // Sample from bottom edge, left to right
                    (segment_start, height - edge_depth, segment_end, height)
                }
                Edge::Left => {
                    // Sample from left edge, top to bottom
                    (0, segment_start, edge_depth, segment_end)
                }
                Edge::Right => {
                    // Sample from right edge, top to bottom
                    (width - edge_depth, segment_start, width, segment_end)
                }
            };

            let color = Self::average_color_in_region(img, x_start, y_start, x_end, y_end);
            samples.push(color);
        }

        ColorSpectrum::new(samples)
    }

    /// Calculate average color in a rectangular region
    fn average_color_in_region(
        img: &ImageBuffer<Rgb<u8>, Vec<u8>>,
        x_start: u32,
        y_start: u32,
        x_end: u32,
        y_end: u32,
    ) -> Color {
        let mut r_sum: u64 = 0;
        let mut g_sum: u64 = 0;
        let mut b_sum: u64 = 0;
        let mut count: u64 = 0;

        // Sample every Nth pixel for performance (adjust based on need)
        let sample_step = 4;

        for y in (y_start..y_end).step_by(sample_step) {
            for x in (x_start..x_end).step_by(sample_step) {
                if let Some(pixel) = img.get_pixel_checked(x, y) {
                    r_sum += pixel[0] as u64;
                    g_sum += pixel[1] as u64;
                    b_sum += pixel[2] as u64;
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