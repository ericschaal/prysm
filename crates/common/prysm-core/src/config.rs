/// Controls ambient light processing. Start with [`Config::default()`]
/// and change only the settings you need; black-bar detection uses built-in tuning.
///
/// ```
/// use prysm_core::Config;
///
/// let config = Config {
///     brightness_percent: prysm_core::BrightnessPercent(60),
///     smoothing_seconds: prysm_core::SmoothingSeconds(0.2),
///     sample_density: prysm_core::SampleDensity(150),
///     ..Config::default()
/// };
/// ```
/// Percentages for different settings cannot be interchanged:
///
/// ```compile_fail
/// let config = prysm_core::Config {
///     brightness_percent: prysm_core::SpatialReachPercent(60),
///     ..prysm_core::Config::default()
/// };
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Config {
    /// Light output: 0 = off, 100 = full brightness. Default: 100.
    /// Values above 100 are treated as 100.
    pub brightness_percent: BrightnessPercent,

    /// Boost dark colors without lighting black. Default: 0; clamped to 0–100.
    pub shadow_lift_percent: ShadowLiftPercent,

    /// Seconds to complete 95% of a color transition, independent of frame rate.
    /// Higher values reduce flicker but make the lights respond more slowly.
    /// Default: 0.1. Zero, negative, and non-finite values disable smoothing.
    pub smoothing_seconds: SmoothingSeconds,

    /// Color samples per 1000 pixels of cropped edge length. Default: 128.
    /// Controls spatial detail independently of LED count; all picture pixels
    /// still contribute. Each edge has at least one sample and at most one per pixel.
    pub sample_density: SampleDensity,

    /// Broad spatial influence radius, as a percentage of picture height after
    /// cropping. A local peak uses one quarter of this radius. Default: 10; clamped to 1–100.
    /// Higher values spread colors farther. Every picture pixel contributes.
    pub spatial_reach_percent: SpatialReachPercent,

    /// Follow the picture inside black bars instead of making the lights dark.
    /// Default: true. Cropping waits for stable bars to avoid flicker.
    pub remove_black_bars: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            brightness_percent: BrightnessPercent(100),
            shadow_lift_percent: ShadowLiftPercent(0),
            smoothing_seconds: SmoothingSeconds(0.1),
            sample_density: SampleDensity(128),
            spatial_reach_percent: SpatialReachPercent(10),
            remove_black_bars: true,
        }
    }
}

/// Light output in percent; values above 100 use full brightness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrightnessPercent(pub u8);

impl BrightnessPercent {
    #[must_use]
    pub fn as_ratio(self) -> f32 {
        f32::from(self.0.min(100)) / 100.0
    }
}

impl Default for BrightnessPercent {
    fn default() -> Self {
        Config::default().brightness_percent
    }
}

/// Spatial influence radius in percent of picture height, clamped to 1–100.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpatialReachPercent(pub u8);

impl SpatialReachPercent {
    #[must_use]
    pub fn as_ratio(self) -> f32 {
        f32::from(self.0.clamp(1, 100)) / 100.0
    }
}

impl Default for SpatialReachPercent {
    fn default() -> Self {
        Config::default().spatial_reach_percent
    }
}

/// Seconds to complete 95% of a transition; non-positive or non-finite values disable smoothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmoothingSeconds(pub f32);

impl SmoothingSeconds {
    #[must_use]
    pub fn is_enabled(self) -> bool {
        self.0.is_finite() && self.0 > 0.0
    }
}

/// Shadow boost strength; zero preserves the original output.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ShadowLiftPercent(pub u8);

/// Sample density representing samples per 1000 pixels of edge length
///
/// Example: `SampleDensity(50)` means 50 samples per 1000px, so a 1920px edge gets ~96 samples
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleDensity(pub usize);

impl SampleDensity {
    /// Calculate samples, capped at one per pixel (at least one for empty edges).
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "sample counts use approximate geometry and saturate to the edge length"
    )]
    pub fn samples_for_length(self, length_px: usize) -> usize {
        (((length_px as f32 / 1000.0) * self.0 as f32).max(1.0) as usize).min(length_px.max(1))
    }

    /// Get the raw density value (samples per 1000px)
    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

impl Default for SampleDensity {
    fn default() -> Self {
        Config::default().sample_density
    }
}
