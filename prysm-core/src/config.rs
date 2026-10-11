use crate::SampleDensity;

/// Controls how the ambient lights look. Start with [`Config::default()`]
/// and change only the settings you need; black-bar detection uses built-in tuning.
///
/// ```
/// use prysm_core::Config;
///
/// let config = Config {
///     brightness_percent: 60,
///     smoothing_seconds: 0.2,
///     sample_density: prysm_core::SampleDensity(150),
///     ..Config::default()
/// };
/// ```
#[derive(Debug, Clone)]
pub struct Config {
    /// Light output: 0 = off, 100 = full brightness. Default: 80.
    /// Values above 100 are treated as 100.
    pub brightness_percent: u8,

    /// Seconds to complete 95% of a color transition, independent of frame rate.
    /// Higher values reduce flicker but make the lights respond more slowly.
    /// Default: 0.1. Zero, negative, and non-finite values disable smoothing.
    pub smoothing_seconds: f32,

    /// Color samples per 1000 pixels of cropped edge length. Default: 60.
    /// Controls spatial detail independently of LED count; all region pixels
    /// still contribute. Each edge has at least one sample and at most one per pixel.
    pub sample_density: SampleDensity,

    /// How far inward to read colors from each edge, as a percentage of the
    /// picture height after black bars are removed. Default: 15.
    /// Colors have strongest influence at the edge, fading quadratically toward
    /// the interior. Clamped to 1–50, with opposite edges never overlapping.
    pub edge_depth_percent: u8,

    /// Follow the picture inside black bars instead of making the lights dark.
    /// Default: true. Cropping waits for stable bars to avoid flicker.
    pub remove_black_bars: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            brightness_percent: 80,
            smoothing_seconds: 0.1,
            sample_density: SampleDensity::default(),
            edge_depth_percent: 15,
            remove_black_bars: true,
        }
    }
}
