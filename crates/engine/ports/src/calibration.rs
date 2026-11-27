use prysm_core::{
    ColorCorrection, Config, DarkFloorPercent, SaturationBoostPercent, SmoothingSeconds,
};
use serde::{Deserialize, Serialize};

/// The calibration pattern a TV is showing, so the lights can be compared
/// with it while calibrations are edited. Engine state like the target:
/// never persisted and not part of [`Settings`](crate::Settings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(remote = "Self", deny_unknown_fields)]
pub struct CalibrationPattern {
    pub pattern: Pattern,
    /// Apply no calibration while the pattern shows, to compare against
    /// the uncorrected colors.
    pub uncorrected: bool,
}
crate::map_only!(CalibrationPattern, "a calibration pattern as an object");

impl CalibrationPattern {
    /// The processing config while this pattern shows: no smoothing, so a
    /// change shows at once, and with `uncorrected` no color correction.
    /// Colour boost and the dark floor are off, so a calibration matches the
    /// lights to the screen itself and holds whatever style is chosen later.
    /// Brightness and black bars stay as configured.
    #[must_use]
    pub fn adjust(self, config: Config) -> Config {
        Config {
            smoothing_seconds: SmoothingSeconds(0.0),
            saturation_boost_percent: SaturationBoostPercent(0),
            dark_floor_percent: DarkFloorPercent(0),
            correction: if self.uncorrected {
                ColorCorrection::default()
            } else {
                config.correction
            },
            ..config
        }
    }
}

/// A calibration pattern, by its TV clip name.
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::VariantArray)]
#[serde(rename_all = "snake_case")]
pub enum Pattern {
    /// Light gray (192) full screen: white balance (warmth, tint).
    #[serde(rename = "gray_192")]
    Gray192,
    /// Full-screen primaries and secondaries, one colour each. The
    /// secondaries check the primaries they mix.
    Red,
    Yellow,
    Green,
    Cyan,
    Blue,
    Magenta,
    /// Moderate blue left, vivid blue right: the blue primary's controls.
    BluePair,
    /// Moderate red left, vivid red right: the red primary's controls.
    RedPair,
    /// Moderate green left, vivid green right: the green primary's controls.
    GreenPair,
    /// Peach left, teal right: mixed colors.
    Coast,
    /// Rose left, sage right: mixed colors.
    Garden,
    /// Amber left, violet right: mixed colors.
    Dusk,
    /// A grid of colors to review the calibration at each corner.
    ReviewGrid,
}

impl Pattern {
    pub const ALL: [Self; 14] = *<Self as strum::VariantArray>::VARIANTS.as_array().unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_core::BrightnessPercent;

    #[test]
    fn only_an_object_is_a_calibration_pattern() {
        let read = serde_json::from_value::<CalibrationPattern>;
        let object = serde_json::json!({ "pattern": "red", "uncorrected": false });
        assert_eq!(
            read(object).unwrap(),
            CalibrationPattern {
                pattern: Pattern::Red,
                uncorrected: false
            }
        );
        let error = read(serde_json::json!(["red", false]))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("a calibration pattern as an object"),
            "{error}"
        );
    }

    #[test]
    fn patterns_use_the_tv_clip_names() {
        let names: Vec<_> = Pattern::ALL
            .iter()
            .map(|pattern| serde_json::to_value(pattern).unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "gray_192",
                "red",
                "yellow",
                "green",
                "cyan",
                "blue",
                "magenta",
                "blue_pair",
                "red_pair",
                "green_pair",
                "coast",
                "garden",
                "dusk",
                "review_grid"
            ]
        );
        for pattern in Pattern::ALL {
            let json = serde_json::to_value(pattern).unwrap();
            assert_eq!(serde_json::from_value::<Pattern>(json).unwrap(), pattern);
        }
        for dropped in [
            "gray-64",
            "gray-192",
            "blue-pair",
            "response",
            "colors",
            "Red",
        ] {
            assert!(serde_json::from_value::<Pattern>(dropped.into()).is_err());
        }
    }

    #[test]
    fn a_pattern_shows_without_smoothing_boost_or_glow_and_optionally_without_correction() {
        let configured = Config {
            correction: ColorCorrection::new(40, 0, 0).unwrap(),
            brightness_percent: BrightnessPercent(55),
            smoothing_seconds: SmoothingSeconds(2.5),
            saturation_boost_percent: SaturationBoostPercent(30),
            dark_floor_percent: DarkFloorPercent(10),
            remove_black_bars: true,
            ..Config::default()
        };
        let corrected = CalibrationPattern {
            pattern: Pattern::Gray192,
            uncorrected: false,
        }
        .adjust(configured);
        let plain = Config {
            smoothing_seconds: SmoothingSeconds(0.0),
            saturation_boost_percent: SaturationBoostPercent(0),
            dark_floor_percent: DarkFloorPercent(0),
            ..configured
        };
        assert_eq!(corrected, plain);
        let uncorrected = CalibrationPattern {
            pattern: Pattern::Gray192,
            uncorrected: true,
        }
        .adjust(configured);
        assert_eq!(
            uncorrected,
            Config {
                correction: ColorCorrection::default(),
                ..plain
            }
        );
        assert!(
            serde_json::from_value::<CalibrationPattern>(
                serde_json::json!({ "pattern": "red", "uncorrected": false, "x": 1 })
            )
            .is_err()
        );
    }
}
