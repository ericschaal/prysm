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

    /// Pixel brightness threshold (0-255) for "black"
    pub black_threshold: u8,

    /// Minimum band size in pixels
    pub min_band_size: u32,

    /// Frames between detection scans
    pub band_detection_interval: u32,

    /// Stability frames before applying change
    pub band_stability_frames: u32,
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
            black_threshold: 30,
            min_band_size: 50,
            band_detection_interval: 15,
            band_stability_frames: 3,
        }
    }
}
