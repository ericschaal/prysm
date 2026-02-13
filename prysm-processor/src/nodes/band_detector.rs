use crate::frames::{ColorFrame, Viewport};
use crate::pipeline::Node;
use prysm_core::{Color, Config};

/// Detects black bands and adjusts viewport
#[derive(Debug)]
pub struct BandDetector {
    black_threshold: u8,
    min_band_size: u32,
    detection_interval: u32,
    stability_frames: u32,

    // State
    frame_count: u32,
    current_viewport: Option<Viewport>,
    pending_viewport: Option<(Viewport, u32)>,
}

impl BandDetector {
    pub fn new(config: &Config) -> Self {
        Self {
            black_threshold: config.black_threshold,
            min_band_size: config.min_band_size,
            detection_interval: config.band_detection_interval,
            stability_frames: config.band_stability_frames,
            frame_count: 0,
            current_viewport: None,
            pending_viewport: None,
        }
    }

    /// Detect black bands by scanning edges
    fn detect_viewport(&self, frame: &ColorFrame) -> Viewport {
        let width = frame.width;
        let height = frame.height;

        // Scan from each edge inward until content found
        let top_offset = self.scan_top_band(frame);
        let bottom_offset = self.scan_bottom_band(frame);
        let left_offset = self.scan_left_band(frame);
        let right_offset = self.scan_right_band(frame);

        Viewport {
            x: left_offset,
            y: top_offset,
            width: width - left_offset - right_offset,
            height: height - top_offset - bottom_offset,
        }
    }

    fn scan_top_band(&self, frame: &ColorFrame) -> u32 {
        // Scan rows from top downward
        for y in 0..(frame.height / 2) {
            if !self.is_black_row(frame, y) {
                return y;
            }
        }
        0
    }

    fn scan_bottom_band(&self, frame: &ColorFrame) -> u32 {
        // Scan rows from bottom upward
        for y in (frame.height / 2..frame.height).rev() {
            if !self.is_black_row(frame, y) {
                return frame.height - y - 1;
            }
        }
        0
    }

    fn scan_left_band(&self, frame: &ColorFrame) -> u32 {
        // Scan columns from left rightward
        for x in 0..(frame.width / 2) {
            if !self.is_black_column(frame, x) {
                return x;
            }
        }
        0
    }

    fn scan_right_band(&self, frame: &ColorFrame) -> u32 {
        // Scan columns from right leftward
        for x in (frame.width / 2..frame.width).rev() {
            if !self.is_black_column(frame, x) {
                return frame.width - x - 1;
            }
        }
        0
    }

    fn is_black_row(&self, frame: &ColorFrame, y: u32) -> bool {
        // Sparse sampling: every 16th pixel
        let mut black_count = 0;
        let mut total_count = 0;

        for x in (0..frame.width).step_by(16) {
            let idx = (y * frame.width + x) as usize;
            if let Some(color) = frame.pixels.get(idx) {
                if self.is_black_pixel(*color) {
                    black_count += 1;
                }
                total_count += 1;
            }
        }

        // Row is black if 90%+ of sampled pixels are black
        total_count > 0 && (black_count as f32 / total_count as f32) >= 0.9
    }

    fn is_black_column(&self, frame: &ColorFrame, x: u32) -> bool {
        // Sparse sampling: every 16th pixel
        let mut black_count = 0;
        let mut total_count = 0;

        for y in (0..frame.height).step_by(16) {
            let idx = (y * frame.width + x) as usize;
            if let Some(color) = frame.pixels.get(idx) {
                if self.is_black_pixel(*color) {
                    black_count += 1;
                }
                total_count += 1;
            }
        }

        total_count > 0 && (black_count as f32 / total_count as f32) >= 0.9
    }

    fn is_black_pixel(&self, color: Color) -> bool {
        color.r <= self.black_threshold
            && color.g <= self.black_threshold
            && color.b <= self.black_threshold
    }

    fn apply_with_stability(&mut self, detected: Viewport) -> Viewport {
        match &mut self.pending_viewport {
            Some((pending, count)) if *pending == detected => {
                *count += 1;
                if *count >= self.stability_frames {
                    self.current_viewport = Some(detected);
                    self.pending_viewport = None;
                }
            }
            Some(_) => {
                self.pending_viewport = Some((detected, 1));
            }
            None => {
                if Some(detected) != self.current_viewport {
                    self.pending_viewport = Some((detected, 1));
                }
            }
        }

        self.current_viewport
            .unwrap_or_else(|| Viewport::full_frame(0, 0)) // Will be overridden
    }
}

impl Node<ColorFrame, ColorFrame> for BandDetector {
    fn process(&mut self, mut input: ColorFrame) -> ColorFrame {
        self.frame_count += 1;

        if self.frame_count % self.detection_interval == 0 {
            let detected = self.detect_viewport(&input);
            input.viewport = self.apply_with_stability(detected);
        } else if let Some(vp) = self.current_viewport {
            input.viewport = vp;
        }

        input
    }
}
