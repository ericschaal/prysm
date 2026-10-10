/// Controls how the ambient lights look. Start with [`Config::default()`]
/// and change only the settings you need; sampling and black-bar detection
/// use built-in tuning.
///
/// ```
/// use prysm_core::Config;
///
/// let config = Config {
///     brightness_percent: 60,
///     smoothing_percent: 70,
///     ..Config::default()
/// };
/// ```
#[derive(Debug, Clone)]
pub struct Config {
    /// Light output: 0 = off, 100 = full brightness. Default: 80.
    /// Values above 100 are treated as 100.
    pub brightness_percent: u8,

    /// Smoothness of color transitions: 0 = instant, 100 = slowest.
    /// Higher values reduce flicker but make the lights respond more slowly.
    /// Default: 40. Values above 100 are treated as 100.
    pub smoothing_percent: u8,

    /// How far inward to read colors from each edge, as a percentage of the
    /// picture height after black bars are removed. Default: 3.
    /// Smaller values follow the outer edge; larger values mix in more of the
    /// picture. Clamped to 1–50, and limited so opposite edges never overlap.
    pub edge_depth_percent: u8,

    /// Follow the picture inside black bars instead of making the lights dark.
    /// Default: true. Cropping waits for stable bars to avoid flicker.
    pub remove_black_bars: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            brightness_percent: 80,
            smoothing_percent: 40,
            // Keep objects near the border from being diluted by the interior.
            edge_depth_percent: 3,
            remove_black_bars: true,
        }
    }
}
