#![no_main]

#[derive(Debug, Clone)]
pub struct Config {
    /// Number of LEDs per zone
    pub leds_per_zone: usize,

    /// Total number of LEDs
    pub total_leds: usize,

    /// Brightness (0.0 to 1.0)
    pub brightness: f32,

    /// Smoothing factor for color transitions (0.0 = no smoothing, 1.0 = maximum smoothing)
    pub smoothing: f32,

    /// Depth of the edge sampling (as fraction of screen, e.g., 0.1 = 10% from edge)
    pub edge_depth: f32,

    /// Target FPS
    pub target_fps: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            leds_per_zone: 20,
            total_leds: 160,
            brightness: 0.8,
            smoothing: 0.7,
            edge_depth: 0.15,
            target_fps: 30,
        }
    }
}