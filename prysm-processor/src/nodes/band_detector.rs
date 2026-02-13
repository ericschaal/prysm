use crate::frames::{ColorFrame, Viewport};
use crate::pipeline::Node;
use prysm_core::{Color, Config};

/// Detects black bands using histogram projection algorithm
#[derive(Debug)]
pub struct BandDetector {
    // Config
    brightness_percentile: u8,
    min_band_size: u32,
    detection_interval: u32,
    temporal_smoothing: f32,
    sample_stride: u32,

    // State
    frame_count: u32,
    smoothed_viewport: Option<Viewport>,
}

impl BandDetector {
    pub fn new(config: &Config) -> Self {
        Self {
            brightness_percentile: config.band_brightness_percentile,
            min_band_size: config.min_band_size,
            detection_interval: config.band_detection_interval,
            temporal_smoothing: config.band_temporal_smoothing,
            sample_stride: config.band_sample_stride,
            frame_count: 0,
            smoothed_viewport: None,
        }
    }

    /// Detect viewport by analyzing brightness projections
    fn detect_viewport(&self, frame: &ColorFrame) -> Viewport {
        // Build brightness projections
        let row_brightness = self.build_row_projection(frame);
        let col_brightness = self.build_col_projection(frame);

        // Calculate adaptive thresholds
        let row_threshold = self.calculate_percentile_threshold(&row_brightness);
        let col_threshold = self.calculate_percentile_threshold(&col_brightness);

        // Find continuous band regions from edges
        let top = self.find_band_from_start(&row_brightness, row_threshold);
        let bottom = self.find_band_from_end(&row_brightness, row_threshold);
        let left = self.find_band_from_start(&col_brightness, col_threshold);
        let right = self.find_band_from_end(&col_brightness, col_threshold);

        Viewport {
            x: left,
            y: top,
            width: frame.width.saturating_sub(left + right),
            height: frame.height.saturating_sub(top + bottom),
        }
    }

    /// Build brightness projection for each row (horizontal projection)
    fn build_row_projection(&self, frame: &ColorFrame) -> Vec<u8> {
        let mut projection = Vec::with_capacity(frame.height as usize);

        for y in 0..frame.height {
            // Sample every Nth pixel in this row
            let samples: Vec<Color> = (0..frame.width)
                .step_by(self.sample_stride as usize)
                .filter_map(|x| {
                    let idx = (y * frame.width + x) as usize;
                    frame.pixels.get(idx).copied()
                })
                .collect();

            projection.push(median_brightness(&samples));
        }

        projection
    }

    /// Build brightness projection for each column (vertical projection)
    fn build_col_projection(&self, frame: &ColorFrame) -> Vec<u8> {
        let mut projection = Vec::with_capacity(frame.width as usize);

        for x in 0..frame.width {
            // Sample every Nth pixel in this column
            let samples: Vec<Color> = (0..frame.height)
                .step_by(self.sample_stride as usize)
                .filter_map(|y| {
                    let idx = (y * frame.width + x) as usize;
                    frame.pixels.get(idx).copied()
                })
                .collect();

            projection.push(median_brightness(&samples));
        }

        projection
    }

    /// Calculate percentile threshold from brightness distribution
    fn calculate_percentile_threshold(&self, brightness: &[u8]) -> u8 {
        if brightness.is_empty() {
            return 0;
        }

        let mut sorted = brightness.to_vec();
        sorted.sort_unstable();

        let index =
            ((brightness.len() as f32 * self.brightness_percentile as f32) / 100.0) as usize;
        let percentile_value = sorted[index.min(sorted.len() - 1)];

        // Cap threshold at 50 to prevent uniform content from being detected as bands
        // This ensures only genuinely dark regions are considered as bands
        percentile_value.min(50)
    }

    /// Find continuous low-brightness band from start of array
    fn find_band_from_start(&self, brightness: &[u8], threshold: u8) -> u32 {
        let mut band_size = 0;

        for &b in brightness.iter() {
            if b <= threshold {
                band_size += 1;
            } else {
                // First non-black pixel found - stop
                break;
            }
        }

        // Only return if meets minimum size
        if band_size >= self.min_band_size {
            band_size
        } else {
            0
        }
    }

    /// Find continuous low-brightness band from end of array
    fn find_band_from_end(&self, brightness: &[u8], threshold: u8) -> u32 {
        let mut band_size = 0;

        for &b in brightness.iter().rev() {
            if b <= threshold {
                band_size += 1;
            } else {
                // First non-black pixel found - stop
                break;
            }
        }

        // Only return if meets minimum size
        if band_size >= self.min_band_size {
            band_size
        } else {
            0
        }
    }

    /// Apply exponential moving average smoothing to viewport transitions
    fn apply_temporal_smoothing(&mut self, detected: Viewport) -> Viewport {
        match self.smoothed_viewport {
            Some(prev) => {
                let alpha = 1.0 - self.temporal_smoothing;

                Viewport {
                    x: (prev.x as f32 * self.temporal_smoothing + detected.x as f32 * alpha)
                        as u32,
                    y: (prev.y as f32 * self.temporal_smoothing + detected.y as f32 * alpha)
                        as u32,
                    width: (prev.width as f32 * self.temporal_smoothing
                        + detected.width as f32 * alpha) as u32,
                    height: (prev.height as f32 * self.temporal_smoothing
                        + detected.height as f32 * alpha) as u32,
                }
            }
            None => detected,
        }
    }
}

impl Node<ColorFrame, ColorFrame> for BandDetector {
    fn process(&mut self, mut input: ColorFrame) -> ColorFrame {
        self.frame_count += 1;

        if self.frame_count % self.detection_interval == 0 {
            // Run detection
            let detected = self.detect_viewport(&input);

            // Apply temporal smoothing
            let smoothed = self.apply_temporal_smoothing(detected);
            self.smoothed_viewport = Some(smoothed);

            input.viewport = smoothed;
        } else if let Some(vp) = self.smoothed_viewport {
            // Use cached viewport
            input.viewport = vp;
        }

        input
    }
}

/// Calculate median brightness of pixel samples
fn median_brightness(pixels: &[Color]) -> u8 {
    if pixels.is_empty() {
        return 0;
    }

    // Calculate brightness (simple average of RGB)
    let mut brightnesses: Vec<u8> = pixels
        .iter()
        .map(|c| ((c.r as u16 + c.g as u16 + c.b as u16) / 3) as u8)
        .collect();

    // Find median using partial sort (faster than full sort)
    let mid = brightnesses.len() / 2;
    brightnesses.select_nth_unstable(mid);
    brightnesses[mid]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create test frame with letterboxing
    fn create_letterboxed_frame(
        width: u32,
        height: u32,
        top_band: u32,
        bottom_band: u32,
    ) -> ColorFrame {
        let mut pixels = vec![Color::black(); (width * height) as usize];

        // Fill content area with gray
        for y in top_band..(height - bottom_band) {
            for x in 0..width {
                let idx = (y * width + x) as usize;
                pixels[idx] = Color::new(128, 128, 128);
            }
        }

        ColorFrame::new(pixels, width, height)
    }

    /// Create test frame with pillarboxing
    fn create_pillarboxed_frame(
        width: u32,
        height: u32,
        left_band: u32,
        right_band: u32,
    ) -> ColorFrame {
        let mut pixels = vec![Color::black(); (width * height) as usize];

        // Fill content area with gray
        for y in 0..height {
            for x in left_band..(width - right_band) {
                let idx = (y * width + x) as usize;
                pixels[idx] = Color::new(128, 128, 128);
            }
        }

        ColorFrame::new(pixels, width, height)
    }

    /// Create test frame with subtitle in black band
    fn create_frame_with_subtitles(width: u32, height: u32, subtitle_row: u32) -> ColorFrame {
        let mut pixels = vec![Color::black(); (width * height) as usize];

        // Top band: 100px black
        // Content: gray
        // Bottom band: 100px black with white subtitle

        // Content area
        for y in 100..(height - 100) {
            for x in 0..width {
                let idx = (y * width + x) as usize;
                pixels[idx] = Color::new(128, 128, 128);
            }
        }

        // Subtitle: white text in bottom black band
        for x in (width / 4)..(3 * width / 4) {
            let idx = (subtitle_row * width + x) as usize;
            pixels[idx] = Color::new(255, 255, 255);
        }

        ColorFrame::new(pixels, width, height)
    }

    #[test]
    fn test_median_brightness() {
        // Odd number of samples
        let pixels = vec![
            Color::new(0, 0, 0),       // brightness: 0
            Color::new(100, 100, 100), // brightness: 100
            Color::new(200, 200, 200), // brightness: 200
        ];
        assert_eq!(median_brightness(&pixels), 100);

        // Even number of samples
        let pixels = vec![
            Color::new(0, 0, 0),
            Color::new(100, 100, 100),
            Color::new(200, 200, 200),
            Color::new(255, 255, 255),
        ];
        let median = median_brightness(&pixels);
        assert!(median == 100 || median == 200); // Either is valid for even length

        // Empty array
        assert_eq!(median_brightness(&[]), 0);

        // Single element
        let pixels = vec![Color::new(128, 128, 128)];
        assert_eq!(median_brightness(&pixels), 128);
    }

    #[test]
    fn test_percentile_threshold() {
        let mut config = Config::default();
        config.band_brightness_percentile = 15; // Use 15% for this test
        let detector = BandDetector::new(&config);

        // Test with known distribution
        let brightness = vec![0, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100];

        // 15th percentile should be around index 1-2
        let threshold = detector.calculate_percentile_threshold(&brightness);
        assert!(threshold <= 20);

        // All black
        let brightness = vec![0; 100];
        assert_eq!(detector.calculate_percentile_threshold(&brightness), 0);

        // All white (should be capped at 50)
        let brightness = vec![255; 100];
        assert_eq!(detector.calculate_percentile_threshold(&brightness), 50);
    }

    #[test]
    fn test_band_boundary_detection() {
        let config = Config::default();
        let detector = BandDetector::new(&config);

        // Continuous black region larger than minimum
        let brightness = vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 10 black
                             0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 20 black
                             0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 30 black
                             0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 40 black
                             0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 50 black
                             100, 100, 100]; // Content starts
        assert_eq!(detector.find_band_from_start(&brightness, 10), 50);

        // Continuous black region smaller than minimum
        let brightness = vec![0, 0, 0, 0, 0, 100, 100, 100];
        assert_eq!(detector.find_band_from_start(&brightness, 10), 0);

        // Discontinuous (subtitle case)
        let brightness = vec![0, 0, 0, 0, 200, 0, 0, 0]; // White pixel breaks continuity
        assert_eq!(detector.find_band_from_start(&brightness, 10), 0);

        // From end - need at least 50 zeros for minimum band size
        let brightness = vec![100, 100, 100, 0, 0, 0, 0, 0,
                             0, 0, 0, 0, 0, 0, 0, 0,
                             0, 0, 0, 0, 0, 0, 0, 0,
                             0, 0, 0, 0, 0, 0, 0, 0,
                             0, 0, 0, 0, 0, 0, 0, 0,
                             0, 0, 0, 0, 0, 0, 0, 0,
                             0, 0, 0, 0, 0]; // 50 zeros total
        assert_eq!(detector.find_band_from_end(&brightness, 10), 50);
    }

    #[test]
    fn test_letterbox_detection() {
        let config = Config::default();
        let detector = BandDetector::new(&config);

        // Create 1920x1080 frame with 240px bands top/bottom (2.35:1 aspect ratio)
        let frame = create_letterboxed_frame(1920, 1080, 240, 240);

        let viewport = detector.detect_viewport(&frame);

        // Should detect both bands (allowing some tolerance)
        assert!(viewport.y >= 230 && viewport.y <= 250, "Top band: {}", viewport.y);
        assert!(
            viewport.height >= 580 && viewport.height <= 620,
            "Height: {}",
            viewport.height
        );
        assert_eq!(viewport.x, 0);
        assert_eq!(viewport.width, 1920);
    }

    #[test]
    fn test_pillarbox_detection() {
        let config = Config::default();
        let detector = BandDetector::new(&config);

        // Create frame with 240px bands left/right
        let frame = create_pillarboxed_frame(1920, 1080, 240, 240);

        let viewport = detector.detect_viewport(&frame);

        // Should detect both bands
        assert!(viewport.x >= 230 && viewport.x <= 250, "Left band: {}", viewport.x);
        assert!(
            viewport.width >= 1420 && viewport.width <= 1460,
            "Width: {}",
            viewport.width
        );
        assert_eq!(viewport.y, 0);
        assert_eq!(viewport.height, 1080);
    }

    #[test]
    fn test_subtitle_handling() {
        let config = Config::default();
        let detector = BandDetector::new(&config);

        // Frame with subtitle in bottom black band
        let frame = create_frame_with_subtitles(1920, 1080, 1000);

        let viewport = detector.detect_viewport(&frame);

        // Should still detect top band correctly
        assert!(viewport.y >= 90 && viewport.y <= 110, "Top band: {}", viewport.y);

        // Bottom band detection should stop at subtitle or detect reduced band
        // The exact behavior depends on subtitle density, but should not crash
        assert!(viewport.height > 0 && viewport.height <= 980);
    }

    #[test]
    fn test_no_letterbox() {
        let config = Config::default();
        let detector = BandDetector::new(&config);

        // Full frame of gray content
        let pixels = vec![Color::new(128, 128, 128); (1920 * 1080) as usize];
        let frame = ColorFrame::new(pixels, 1920, 1080);

        let viewport = detector.detect_viewport(&frame);

        // Should detect no bands
        assert_eq!(viewport.x, 0);
        assert_eq!(viewport.y, 0);
        assert_eq!(viewport.width, 1920);
        assert_eq!(viewport.height, 1080);
    }

    #[test]
    fn test_temporal_smoothing() {
        let mut config = Config::default();
        config.band_temporal_smoothing = 0.5; // 50% smoothing
        let mut detector = BandDetector::new(&config);

        let viewport1 = Viewport {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let viewport2 = Viewport {
            x: 0,
            y: 100,
            width: 1920,
            height: 880,
        };

        // First detection: no previous viewport
        let smoothed = detector.apply_temporal_smoothing(viewport1);
        assert_eq!(smoothed.y, 0);

        // Update internal state to simulate first detection
        detector.smoothed_viewport = Some(viewport1);

        // Second detection: should be halfway between
        let smoothed = detector.apply_temporal_smoothing(viewport2);
        assert!(smoothed.y > 0 && smoothed.y < 100);
        assert!(smoothed.y >= 45 && smoothed.y <= 55); // Should be around 50
    }

    #[test]
    fn test_projection_building() {
        let config = Config::default();
        let detector = BandDetector::new(&config);

        let frame = create_letterboxed_frame(1920, 1080, 100, 100);

        let row_projection = detector.build_row_projection(&frame);
        assert_eq!(row_projection.len(), 1080);

        // Top 100 rows should be black (brightness ~0)
        for i in 0..100 {
            assert!(
                row_projection[i] < 20,
                "Row {} brightness: {}",
                i,
                row_projection[i]
            );
        }

        // Middle rows should be gray (brightness ~128)
        for i in 100..980 {
            assert!(
                row_projection[i] > 100,
                "Row {} brightness: {}",
                i,
                row_projection[i]
            );
        }

        // Bottom 100 rows should be black
        for i in 980..1080 {
            assert!(
                row_projection[i] < 20,
                "Row {} brightness: {}",
                i,
                row_projection[i]
            );
        }
    }
}
