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

    /// Depth of the edge sampling region as a fraction of frame height.
    /// Resolution-independent: 0.09 is ~100px at 1080p, ~32px at 360p.
    pub edge_depth: f32,

    /// Target FPS
    pub target_fps: u32,

    /// Enable black band detection
    pub black_band_detection: bool,

    /// Brightness percentile threshold (0-100) for band detection
    /// Example: 15 means use 15th percentile of row/col brightness as threshold
    pub band_brightness_percentile: u8,

    /// Minimum band size as a fraction of the frame dimension the band spans
    pub min_band_fraction: f32,

    /// Frames between detection scans (lower = faster detection)
    pub band_detection_interval: u32,

    /// Frames of consistent detection before applying new viewport
    pub band_confirm_frames: u32,

    /// Frames of different detection before resetting candidate
    pub band_inconsistency_limit: u32,

    /// Sample stride for projection calculation (pixels to skip)
    pub band_sample_stride: u32,

    /// Skip processing when the frame is unchanged from the last processed one
    pub change_detection: bool,

    /// Mean absolute luma delta (0-255 scale) below which a frame counts as unchanged
    pub change_threshold: f32,

    /// Force reprocessing after this many consecutive skipped frames,
    /// so slow fades below the threshold can never wedge the output
    pub max_skipped_frames: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // Per 1000px of edge: 30 gives 19 samples across a 640px edge,
            // matching the gradient resolution 1080p capture had at density 10
            sample_density: SampleDensity(30),
            brightness: 0.8,
            temporal_smoothing: 0.4,
            edge_depth: 0.09,
            target_fps: 30,
            black_band_detection: true,
            band_brightness_percentile: 4,
            min_band_fraction: 0.04,
            band_detection_interval: 4,
            band_confirm_frames: 15,
            band_inconsistency_limit: 5,
            band_sample_stride: 8,
            change_detection: true,
            change_threshold: 1.0,
            max_skipped_frames: 30,
        }
    }
}
