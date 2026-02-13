use prysm_capture::{Frame, PixelFormat};
use prysm_core::{Color, ColorSpectrum, Config, Edge, EdgeSpectrums};

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
        let current_spectrums = match frame.format {
            PixelFormat::RGB24 => self.process_rgb_frame(frame),
            PixelFormat::YUYV => self.process_yuyv_frame(frame),
            PixelFormat::MJPEG | PixelFormat::BGR24 => {
                tracing::error!("{} format not yet supported", frame.format);
                EdgeSpectrums::black(
                    frame.width as usize,
                    frame.height as usize,
                    self.config.samples_per_1000px,
                )
            }
        };

        // Apply temporal smoothing
        let smoothed_spectrums = if let Some(ref prev) = self.previous_spectrums {
            prev.blend(&current_spectrums, 1.0 - self.config.smoothing)
        } else {
            current_spectrums.clone()
        };

        self.previous_spectrums = Some(smoothed_spectrums.clone());
        smoothed_spectrums
    }

    /// Process an RGB24 frame (zero-copy - samples directly from Arc)
    fn process_rgb_frame(&self, frame: &Frame) -> EdgeSpectrums {
        let width = frame.width;
        let height = frame.height;

        // Extract spectrum for each edge using direct RGB sampling (zero-copy)
        let top_spectrum = self.extract_edge_spectrum_rgb(&frame.data, width, height, Edge::Top);
        let right_spectrum =
            self.extract_edge_spectrum_rgb(&frame.data, width, height, Edge::Right);
        let bottom_spectrum =
            self.extract_edge_spectrum_rgb(&frame.data, width, height, Edge::Bottom);
        let left_spectrum = self.extract_edge_spectrum_rgb(&frame.data, width, height, Edge::Left);

        EdgeSpectrums::new(top_spectrum, right_spectrum, bottom_spectrum, left_spectrum)
    }

    /// Process a YUYV frame with inline conversion (only convert sampled pixels)
    fn process_yuyv_frame(&self, frame: &Frame) -> EdgeSpectrums {
        let width = frame.width;
        let height = frame.height;

        // Extract spectrum for each edge using inline YUYV conversion
        let top_spectrum = self.extract_edge_spectrum_yuyv(&frame.data, width, height, Edge::Top);
        let right_spectrum =
            self.extract_edge_spectrum_yuyv(&frame.data, width, height, Edge::Right);
        let bottom_spectrum =
            self.extract_edge_spectrum_yuyv(&frame.data, width, height, Edge::Bottom);
        let left_spectrum = self.extract_edge_spectrum_yuyv(&frame.data, width, height, Edge::Left);

        EdgeSpectrums::new(top_spectrum, right_spectrum, bottom_spectrum, left_spectrum)
    }

    /// Extract color spectrum from a specific edge in RGB24 format (zero-copy)
    fn extract_edge_spectrum_rgb(
        &self,
        rgb_data: &[u8],
        width: u32,
        height: u32,
        edge: Edge,
    ) -> ColorSpectrum {
        // Calculate sample count based on edge length
        let (edge_length, sample_count) = match edge {
            Edge::Top | Edge::Bottom => {
                let length = width;
                let samples = ((length as f32 / 1000.0) * self.config.samples_per_1000px as f32)
                    .max(1.0) as usize;
                (length, samples)
            }
            Edge::Left | Edge::Right => {
                let length = height;
                let samples = ((length as f32 / 1000.0) * self.config.samples_per_1000px as f32)
                    .max(1.0) as usize;
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
            let (x_start, y_start, x_end, y_end) = match edge {
                Edge::Top => (segment_start, 0, segment_end, edge_depth),
                Edge::Bottom => (segment_start, height - edge_depth, segment_end, height),
                Edge::Left => (0, segment_start, edge_depth, segment_end),
                Edge::Right => (width - edge_depth, segment_start, width, segment_end),
            };

            let color =
                Self::average_color_in_region_rgb(rgb_data, width, x_start, y_start, x_end, y_end);
            samples.push(color);
        }

        ColorSpectrum::new(samples)
    }

    /// Extract color spectrum from a specific edge in YUYV format
    fn extract_edge_spectrum_yuyv(
        &self,
        yuyv_data: &[u8],
        width: u32,
        height: u32,
        edge: Edge,
    ) -> ColorSpectrum {
        // Calculate sample count based on edge length
        let (edge_length, sample_count) = match edge {
            Edge::Top | Edge::Bottom => {
                let length = width;
                let samples = ((length as f32 / 1000.0) * self.config.samples_per_1000px as f32)
                    .max(1.0) as usize;
                (length, samples)
            }
            Edge::Left | Edge::Right => {
                let length = height;
                let samples = ((length as f32 / 1000.0) * self.config.samples_per_1000px as f32)
                    .max(1.0) as usize;
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
            let (x_start, y_start, x_end, y_end) = match edge {
                Edge::Top => (segment_start, 0, segment_end, edge_depth),
                Edge::Bottom => (segment_start, height - edge_depth, segment_end, height),
                Edge::Left => (0, segment_start, edge_depth, segment_end),
                Edge::Right => (width - edge_depth, segment_start, width, segment_end),
            };

            let color = Self::average_color_in_region_yuyv(
                yuyv_data, width, x_start, y_start, x_end, y_end,
            );
            samples.push(color);
        }

        ColorSpectrum::new(samples)
    }

    /// Calculate average color in a rectangular region from RGB24 data (zero-copy)
    fn average_color_in_region_rgb(
        rgb_data: &[u8],
        width: u32,
        x_start: u32,
        y_start: u32,
        x_end: u32,
        y_end: u32,
    ) -> Color {
        let mut r_sum: u64 = 0;
        let mut g_sum: u64 = 0;
        let mut b_sum: u64 = 0;
        let mut count: u64 = 0;

        // Sample every Nth pixel for performance (same as other paths)
        let sample_step = 4;

        for y in (y_start..y_end).step_by(sample_step) {
            for x in (x_start..x_end).step_by(sample_step) {
                // RGB24: 3 bytes per pixel [R, G, B]
                let pixel_offset = ((y * width + x) * 3) as usize;

                // Bounds check
                if pixel_offset + 2 >= rgb_data.len() {
                    continue;
                }

                r_sum += rgb_data[pixel_offset] as u64;
                g_sum += rgb_data[pixel_offset + 1] as u64;
                b_sum += rgb_data[pixel_offset + 2] as u64;
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

    /// Calculate average color in a rectangular region from YUYV data with inline conversion
    fn average_color_in_region_yuyv(
        yuyv_data: &[u8],
        width: u32,
        x_start: u32,
        y_start: u32,
        x_end: u32,
        y_end: u32,
    ) -> Color {
        // Fixed-point coefficients (multiplied by 1024) - BT.601 standard
        const V_TO_R: i32 = 1437; // 1.402 * 1024
        const U_TO_G: i32 = 352; // 0.344136 * 1024
        const V_TO_G: i32 = 731; // 0.714136 * 1024
        const U_TO_B: i32 = 1814; // 1.772 * 1024

        let mut r_sum: u64 = 0;
        let mut g_sum: u64 = 0;
        let mut b_sum: u64 = 0;
        let mut count: u64 = 0;

        // Sample every Nth pixel for performance (same as RGB path)
        let sample_step = 4;

        for y in (y_start..y_end).step_by(sample_step) {
            for x in (x_start..x_end).step_by(sample_step) {
                // YUYV stores 2 pixels in 4 bytes: [Y0 U Y1 V]
                // Each pixel pair shares U and V values

                // Calculate offset for the pixel pair containing this pixel
                let pixel_pair_x = (x / 2) * 2; // Align to even pixel
                let yuyv_offset = ((y * width + pixel_pair_x) * 2) as usize;

                // Bounds check
                if yuyv_offset + 3 >= yuyv_data.len() {
                    continue;
                }

                // Extract Y, U, V values
                let y_value = if x % 2 == 0 {
                    yuyv_data[yuyv_offset] as i32 // Y0 for even pixels
                } else {
                    yuyv_data[yuyv_offset + 2] as i32 // Y1 for odd pixels
                };

                let u = yuyv_data[yuyv_offset + 1] as i32 - 128;
                let v = yuyv_data[yuyv_offset + 3] as i32 - 128;

                // Convert YUYV to RGB using BT.601 (inline conversion)
                let r = (y_value + ((v * V_TO_R) >> 10)).clamp(0, 255) as u8;
                let g = (y_value - ((u * U_TO_G + v * V_TO_G) >> 10)).clamp(0, 255) as u8;
                let b = (y_value + ((u * U_TO_B) >> 10)).clamp(0, 255) as u8;

                r_sum += r as u64;
                g_sum += g as u64;
                b_sum += b as u64;
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
