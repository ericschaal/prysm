#[derive(Debug, Clone)]
pub struct Config {
    /// Sample density per 1000 pixels of edge length
    /// Example: 40 samples per 1000px means a 1920px edge gets ~77 samples
    pub samples_per_1000px: usize,

    /// Brightness (0.0 to 1.0)
    pub brightness: f32,

    /// Smoothing factor for color transitions (0.0 = no smoothing, 1.0 = maximum smoothing)
    pub temporal_smoothing: f32,

    /// Depth of the edge sampling in pixels from the screen edge
    pub edge_depth_px: u32,

    pub sample_step: usize,

    /// Target FPS
    pub target_fps: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            samples_per_1000px: 50,
            brightness: 0.8,
            temporal_smoothing: 0.7,
            edge_depth_px: 100,
            sample_step: 10,
            target_fps: 30,
        }
    }
}
