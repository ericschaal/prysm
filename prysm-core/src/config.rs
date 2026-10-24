use crate::SampleDensity;

/// Controls capture resolution and ambient light processing. Start with [`Config::default()`]
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
    /// Requested device capture or decoded file size. Default: 640×360.
    pub capture_resolution: CaptureResolution,

    /// Light output: 0 = off, 100 = full brightness. Default: 100.
    /// Values above 100 are treated as 100.
    pub brightness_percent: BrightnessPercent,

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
            capture_resolution: CaptureResolution([640, 360]),
            brightness_percent: BrightnessPercent(100),
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

/// Positive frame dimensions whose RGB24 buffer size fits in memory indexing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureResolution([u32; 2]);

impl CaptureResolution {
    pub fn new(width: u32, height: u32) -> Result<Self, &'static str> {
        if width == 0 || height == 0 {
            return Err("capture resolution width and height must be positive");
        }
        let bytes = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(3));
        if bytes.is_none_or(|bytes| isize::try_from(bytes).is_err()) {
            return Err("capture resolution is too large");
        }
        Ok(Self([width, height]))
    }

    #[must_use]
    pub const fn dimensions(self) -> [u32; 2] {
        self.0
    }

    #[must_use]
    pub fn rgb24_bytes(self) -> usize {
        self.0[0] as usize * self.0[1] as usize * 3
    }
}

impl Default for CaptureResolution {
    fn default() -> Self {
        Config::default().capture_resolution
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolutions_reject_invalid_dimensions_and_accept_odd_rgb_widths() {
        for (width, height) in [(0, 360), (640, 0), (u32::MAX, u32::MAX)] {
            assert!(CaptureResolution::new(width, height).is_err());
        }
        let resolution = CaptureResolution::new(3, 2).unwrap();
        assert_eq!(resolution.dimensions(), [3, 2]);
        assert_eq!(resolution.rgb24_bytes(), 18);
    }
}
