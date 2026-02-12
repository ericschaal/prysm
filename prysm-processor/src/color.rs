use std::collections::HashMap;
use image::{ImageBuffer, Rgb};
use prysm_capture::Frame;
use prysm_core::{Color, Config, Zone};

#[derive(Debug, Default)]
pub struct ColorProcessor {
    config: Config,
    previous_colors: HashMap<Zone, Color>,
}

impl ColorProcessor {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            previous_colors: HashMap::new(),
        }
    }

    /// Process a frame and extract colors for each zone
    pub fn process_frame(&mut self, frame: &Frame) -> HashMap<Zone, Color> {
        let mut zone_colors = HashMap::new();

        // Convert frame data to image buffer for easier processing
        let img = ImageBuffer::<Rgb<u8>, _>::from_raw(
            frame.width,
            frame.height,
            frame.data.clone(),
        );

        if img.is_none() {
            tracing::error!("Failed to create image buffer from frame");
            return zone_colors;
        }

        let img = img.unwrap();

        for zone in Zone::all() {
            let color = self.extract_zone_color(&img, *zone);

            // Apply smoothing with previous frame
            let smoothed_color = if let Some(prev_color) = self.previous_colors.get(zone) {
                prev_color.blend(&color, 1.0 - self.config.smoothing)
            } else {
                color
            };

            zone_colors.insert(*zone, smoothed_color);
            self.previous_colors.insert(*zone, smoothed_color);
        }

        zone_colors
    }

    /// Extract average color from a specific zone
    fn extract_zone_color(&self, img: &ImageBuffer<Rgb<u8>, Vec<u8>>, zone: Zone) -> Color {
        let (width, height) = img.dimensions();
        let edge_depth = (self.config.edge_depth * width.min(height) as f32) as u32;

        // Define zone boundaries
        let (x_start, y_start, x_end, y_end) = match zone {
            Zone::TopLeft => (0, 0, edge_depth, edge_depth),
            Zone::Top => (edge_depth, 0, width - edge_depth, edge_depth),
            Zone::TopRight => (width - edge_depth, 0, width, edge_depth),
            Zone::Right => (width - edge_depth, edge_depth, width, height - edge_depth),
            Zone::BottomRight => (width - edge_depth, height - edge_depth, width, height),
            Zone::Bottom => (edge_depth, height - edge_depth, width - edge_depth, height),
            Zone::BottomLeft => (0, height - edge_depth, edge_depth, height),
            Zone::Left => (0, edge_depth, edge_depth, height - edge_depth),
        };

        Self::average_color_in_region(img, x_start, y_start, x_end, y_end)
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

    /// Update configuration
    pub fn update_config(&mut self, config: Config) {
        self.config = config;
    }
}