/// Sample density representing samples per 1000 pixels of edge length
///
/// Example: `SampleDensity(50)` means 50 samples per 1000px, so a 1920px edge gets ~96 samples
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleDensity(pub usize);

impl SampleDensity {
    /// Calculate the number of samples for a given edge length in pixels
    #[must_use]
    pub fn samples_for_length(self, length_px: usize) -> usize {
        ((length_px as f32 / 1000.0) * self.0 as f32).max(1.0) as usize
    }

    /// Get the raw density value (samples per 1000px)
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Sample density per 1000 pixels of edge length
    pub sample_density: SampleDensity,

    /// Brightness (0.0 to 1.0)
    pub brightness: f32,

    /// Smoothing factor for color transitions (0.0 = no smoothing, 1.0 = maximum smoothing)
    pub temporal_smoothing: f32,

    /// Depth of the edge sampling in pixels from the screen edge
    pub edge_depth_px: u32,

    pub sample_step: usize,

    /// Target FPS
    pub target_fps: u32,

    /// Enable black band detection
    pub black_band_detection: bool,

    /// Brightness percentile threshold (0-100) for band detection
    /// Example: 15 means use 15th percentile of row/col brightness as threshold
    pub band_brightness_percentile: u8,

    /// Minimum band size in pixels
    pub min_band_size: u32,

    /// Frames between detection scans (lower = faster detection)
    pub band_detection_interval: u32,

    /// Frames of consistent detection before applying new viewport
    pub band_confirm_frames: u32,

    /// Frames of different detection before resetting candidate
    pub band_inconsistency_limit: u32,

    /// Sample stride for projection calculation (pixels to skip)
    pub band_sample_stride: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sample_density: SampleDensity(10),
            brightness: 0.8,
            temporal_smoothing: 0.9,
            edge_depth_px: 100,
            sample_step: 30,
            target_fps: 30,
            black_band_detection: true,
            band_brightness_percentile: 4,
            min_band_size: 50,
            band_detection_interval: 4,
            band_confirm_frames: 15,
            band_inconsistency_limit: 5,
            band_sample_stride: 24,
        }
    }
}
