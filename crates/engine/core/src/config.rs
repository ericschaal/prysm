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
    /// The active calibration's correction, independent of processing controls.
    pub correction: crate::ColorCorrection,
    /// Light output: 0 = off, 100 = full brightness. Default: 100.
    /// Values above 100 are treated as 100.
    pub brightness_percent: BrightnessPercent,

    /// Scales each light's chroma by `1 + p/100`, keeping its luminance and
    /// hue, as far as the lights can show. Default: 0; clamped to 0–100.
    pub saturation_boost_percent: SaturationBoostPercent,

    /// A neutral white glow, in percent of full linear light, that the
    /// lights keep while their colour is darker. Default: 0, fully dark on
    /// black; clamped to 0–20.
    pub dark_floor_percent: DarkFloorPercent,

    /// Seconds to complete 95% of a color transition, independent of frame rate.
    /// Higher values reduce flicker but make the lights respond more slowly.
    /// Default: 0.1. Zero, negative, and non-finite values disable smoothing.
    pub smoothing_seconds: SmoothingSeconds,

    /// How much of a scene cut the lights take at once: 0 eases a cut in like
    /// any other change, 100 shows it in one frame. Ordinary motion is never a
    /// cut. Default: 0; clamped to 0–100.
    pub cut_response_percent: CutResponsePercent,

    /// The least time the lights take from dark to full brightness, whatever
    /// smoothing and cuts allow. Default: 0, no limit.
    pub brightness_ramp_seconds: BrightnessRampSeconds,

    /// Color samples per 1000 pixels of cropped edge length, measured on the
    /// picture scaled down to [`PIXEL_BUDGET`] (640x360): the same value gives
    /// the same samples at any capture size. Default: 128.
    /// Controls spatial detail independently of LED count; all picture pixels
    /// still contribute. Each edge has at least one sample and at most one per
    /// sampling-grid cell along it (at most 128 along the longer side): finer
    /// samples add no detail.
    pub sample_density: SampleDensity,

    /// Broad spatial influence radius, as a percentage of picture height after
    /// cropping. A local peak uses one quarter of this radius. Default: 10; clamped to 1–100.
    /// Higher values spread colors farther. Every picture pixel contributes.
    pub spatial_reach_percent: SpatialReachPercent,

    /// How much each light keeps the hue and saturation next to it, taking
    /// from the rest of the picture only what agrees with it. Brightness
    /// stays that of the plain average. Default: 0, the plain average;
    /// clamped to 0–100.
    pub color_focus_percent: ColorFocusPercent,

    /// Follow the picture inside black bars instead of making the lights dark.
    /// Default: true. Cropping waits for stable bars to avoid flicker.
    pub remove_black_bars: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            correction: crate::ColorCorrection::default(),
            brightness_percent: BrightnessPercent(100),
            saturation_boost_percent: SaturationBoostPercent(0),
            dark_floor_percent: DarkFloorPercent(0),
            smoothing_seconds: SmoothingSeconds(0.1),
            cut_response_percent: CutResponsePercent(0),
            brightness_ramp_seconds: BrightnessRampSeconds(0.0),
            sample_density: SampleDensity(128),
            spatial_reach_percent: SpatialReachPercent(10),
            color_focus_percent: ColorFocusPercent(0),
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

/// How much of a scene cut the lights take at once, in percent, clamped to
/// 0–100; zero eases a cut in like any other change.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CutResponsePercent(pub u8);

impl CutResponsePercent {
    #[must_use]
    pub fn as_ratio(self) -> f32 {
        f32::from(self.0.min(100)) / 100.0
    }
}

/// How strongly each light keeps the colour next to it, in percent, clamped
/// to 0–100; zero is the plain weighted average.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ColorFocusPercent(pub u8);

impl ColorFocusPercent {
    #[must_use]
    pub fn as_ratio(self) -> f32 {
        f32::from(self.0.min(100)) / 100.0
    }
}

/// The least seconds from dark to full brightness; zero, negative and
/// non-finite values set no limit.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct BrightnessRampSeconds(pub f32);

impl BrightnessRampSeconds {
    #[must_use]
    pub fn is_enabled(self) -> bool {
        self.0.is_finite() && self.0 > 0.0
    }
}

/// Chroma boost in percent, clamped to 0–100; zero preserves the colour.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SaturationBoostPercent(pub u8);

impl SaturationBoostPercent {
    #[must_use]
    pub fn as_ratio(self) -> f32 {
        f32::from(self.0.min(100)) / 100.0
    }

    /// Zero: the boost leaves every colour exactly as it is.
    #[must_use]
    pub fn is_neutral(self) -> bool {
        self.0 == 0
    }
}

/// The least luminance the lights keep, in percent of full linear light,
/// clamped to 0–[`DarkFloorPercent::MAX`]; zero lets them go fully dark.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DarkFloorPercent(pub u8);

impl DarkFloorPercent {
    /// More than a faint glow would no longer read as a dark scene.
    pub const MAX: u8 = 20;

    #[must_use]
    pub fn as_ratio(self) -> f32 {
        f32::from(self.0.min(Self::MAX)) / 100.0
    }

    /// Zero: the floor leaves every colour exactly as it is.
    #[must_use]
    pub fn is_neutral(self) -> bool {
        self.0 == 0
    }
}

/// Most pixels sampling sizes itself for: every pixel of a 640x360 picture.
/// A larger picture is sampled as if scaled down to this many pixels
/// ([`budget_size`]), so its cost and its samples do not grow with the capture size.
pub const PIXEL_BUDGET: u64 = 640 * 360;

/// `width` x `height` scaled down, keeping its aspect ratio, to about
/// [`PIXEL_BUDGET`] pixels; a picture within the budget keeps its size.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "scaled sides are below the u32 sides they come from"
)]
pub fn budget_size(width: u32, height: u32) -> [usize; 2] {
    let pixels = u64::from(width) * u64::from(height);
    if pixels <= PIXEL_BUDGET {
        return [width as usize, height as usize];
    }
    let scale = (PIXEL_BUDGET as f64 / pixels as f64).sqrt();
    [width, height].map(|side| (f64::from(side) * scale).round() as usize)
}

/// Samples per 1000 pixels of edge length, measured on the picture scaled
/// down to [`PIXEL_BUDGET`], and at most one per grid cell along the edge
/// ([`Self::samples_for_picture`]).
///
/// Example: `SampleDensity(50)` means 50 samples per 1000px, so the top of a
/// 640x360 picture gets 32 samples, and so does the top of a 1920x1080 one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleDensity(pub usize);

/// The fewest and most grid cells along a picture's longer side.
// shortcut: cap the grid at 128 cells per axis; raise only if footage shows lost detail.
const GRID_CELLS: std::ops::RangeInclusive<usize> = 64..=128;

impl SampleDensity {
    /// Samples along the top and bottom, and along the left and right, of a
    /// `width` x `height` picture: [`Self::samples_for_length`] of its
    /// [`budget_size`], and at most one per grid cell along the edge (the
    /// cells' share of the edge rounded up): finer samples add no detail.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "a cap of at most 128 samples fits usize"
    )]
    pub fn samples_for_picture(self, width: u32, height: u32) -> [usize; 2] {
        let cells = self.grid_cells(width, height);
        let longer = u64::from(width.max(height)).max(1);
        let lengths = budget_size(width, height);
        std::array::from_fn(|axis| {
            let side = u64::from([width, height][axis]);
            let cap = (side * cells).div_ceil(longer).max(1) as usize;
            self.samples_for_length(lengths[axis]).min(cap)
        })
    }

    /// The sampler's grid of a nonempty `width` x `height` picture, columns
    /// and rows: about this density's cells per 1000 px of its
    /// [`budget_size`]'s longer side, 64 to 128 along it, and the other side
    /// in proportion.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "a side's cells are at most the side's pixels"
    )]
    pub fn grid_size(self, width: u32, height: u32) -> [u32; 2] {
        let cells = self.grid_cells(width, height);
        let longer = u64::from(width.max(height));
        [width, height]
            .map(|side| (u64::from(side) * cells / longer).clamp(1, u64::from(side)) as u32)
    }

    /// Grid cells along a picture's longer side.
    fn grid_cells(self, width: u32, height: u32) -> u64 {
        let longer = budget_size(width, height).into_iter().max().unwrap_or(0);
        let cells = self
            .samples_for_length(longer)
            .clamp(*GRID_CELLS.start(), *GRID_CELLS.end());
        u64::try_from(cells).expect("at most 128 cells")
    }

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

    /// Get the raw density value (samples per 1000px at the budget)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cut_response_is_clamped_to_full() {
        for (percent, ratio) in [(0, 0.0), (40, 0.4), (100, 1.0), (101, 1.0), (255, 1.0)] {
            assert_eq!(CutResponsePercent(percent).as_ratio(), ratio);
        }
    }

    #[test]
    fn color_focus_is_clamped_to_full_and_off_by_default() {
        for (percent, ratio) in [(0, 0.0), (50, 0.5), (100, 1.0), (101, 1.0), (255, 1.0)] {
            assert_eq!(ColorFocusPercent(percent).as_ratio(), ratio);
        }
        assert_eq!(Config::default().color_focus_percent.as_ratio(), 0.0);
    }

    #[test]
    fn boost_and_floor_are_clamped_and_off_by_default() {
        for (percent, ratio) in [(0, 0.0), (50, 0.5), (100, 1.0), (101, 1.0), (255, 1.0)] {
            assert_eq!(SaturationBoostPercent(percent).as_ratio(), ratio);
        }
        for (percent, ratio) in [(0, 0.0), (10, 0.1), (20, 0.2), (21, 0.2), (255, 0.2)] {
            assert_eq!(DarkFloorPercent(percent).as_ratio(), ratio);
        }
        let config = Config::default();
        assert_eq!(config.saturation_boost_percent.as_ratio(), 0.0);
        assert_eq!(config.dark_floor_percent.as_ratio(), 0.0);
    }

    #[test]
    fn only_a_finite_positive_ramp_limits_brightness() {
        for seconds in [0.0, -0.5, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(!BrightnessRampSeconds(seconds).is_enabled(), "{seconds}");
        }
        for seconds in [f32::MIN_POSITIVE, 0.5, 2.0] {
            assert!(BrightnessRampSeconds(seconds).is_enabled(), "{seconds}");
        }
    }

    #[test]
    fn pictures_beyond_the_budget_get_the_samples_of_their_640x360_copy() {
        let density = SampleDensity(128);
        assert_eq!(density.samples_for_picture(640, 360), [81, 46]);
        assert_eq!(density.samples_for_picture(360, 640), [46, 81]);
        assert_eq!(density.samples_for_picture(320, 180), [40, 23]);
        for (width, height) in [(1280, 720), (1920, 1080), (3840, 2160)] {
            assert_eq!(budget_size(width, height), [640, 360]);
            assert_eq!(density.samples_for_picture(width, height), [81, 46]);
        }
        assert_eq!(budget_size(0, 2160), [0, 2160]);
        assert_eq!(density.grid_size(640, 360), [81, 45]);
        // At most one sample per grid cell along an edge, at any shape.
        let most = SampleDensity(1024);
        for (width, height) in [(640, 360), (3840, 2160)] {
            assert_eq!(most.samples_for_picture(width, height), [128, 72]);
            assert_eq!(most.grid_size(width, height), [128, 72]);
        }
        assert_eq!(most.samples_for_picture(230_400, 1), [128, 1]);
    }

    #[test]
    fn defaults_take_no_cut_at_once_and_set_no_ramp() {
        let config = Config::default();
        assert_eq!(config.cut_response_percent.as_ratio(), 0.0);
        assert!(!config.brightness_ramp_seconds.is_enabled());
    }
}
