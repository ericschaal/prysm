use prysm_core::{ColorCorrection, Config, SmoothingSeconds};
use serde::{Deserialize, Serialize};

/// The calibration pattern a TV is showing, so the lights can be compared
/// with it while color profiles are edited. Engine state like the target:
/// never persisted and not part of [`Settings`](crate::Settings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Calibration {
    pub pattern: Pattern,
    /// Apply no color profile while the pattern shows, to compare against
    /// the uncorrected colors.
    pub uncorrected: bool,
}

impl Calibration {
    /// The processing config while this pattern shows: no smoothing, so a
    /// change shows at once, and with `uncorrected` no color correction.
    /// Brightness, shadow lift and black bars stay as configured.
    #[must_use]
    pub fn adjust(self, config: Config) -> Config {
        Config {
            smoothing_seconds: SmoothingSeconds(0.0),
            correction: if self.uncorrected {
                ColorCorrection::default()
            } else {
                config.correction
            },
            ..config
        }
    }
}

/// A calibration pattern. The wire names are the TV pattern clips' names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Pattern {
    /// Light gray (192) full screen: white balance (warmth, tint).
    #[serde(rename = "gray-192")]
    Gray192,
    /// Full-screen primaries and secondaries: one hue each.
    Red,
    Yellow,
    Green,
    Cyan,
    /// Shown as [`Pattern::BluePair`] while the blue hue is adjusted.
    Blue,
    Magenta,
    /// Moderate blue left, vivid blue right: intensity, and the blue hue.
    BluePair,
    /// Moderate red left, vivid red right: intensity.
    RedPair,
    /// Moderate green left, vivid green right: intensity.
    GreenPair,
    /// Peach left, teal right: mixed colors.
    Coast,
    /// Rose left, sage right: mixed colors.
    Garden,
    /// Amber left, violet right: mixed colors.
    Dusk,
    /// A grid of colors to review the profile at each corner.
    ReviewGrid,
}

impl Pattern {
    pub const ALL: [Self; 14] = [
        Self::Gray192,
        Self::Red,
        Self::Yellow,
        Self::Green,
        Self::Cyan,
        Self::Blue,
        Self::Magenta,
        Self::BluePair,
        Self::RedPair,
        Self::GreenPair,
        Self::Coast,
        Self::Garden,
        Self::Dusk,
        Self::ReviewGrid,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_core::BrightnessPercent;

    #[test]
    fn patterns_use_the_tv_clip_names() {
        let names: Vec<_> = Pattern::ALL
            .iter()
            .map(|pattern| serde_json::to_value(pattern).unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "gray-192",
                "red",
                "yellow",
                "green",
                "cyan",
                "blue",
                "magenta",
                "blue-pair",
                "red-pair",
                "green-pair",
                "coast",
                "garden",
                "dusk",
                "review-grid"
            ]
        );
        for pattern in Pattern::ALL {
            let json = serde_json::to_value(pattern).unwrap();
            assert_eq!(serde_json::from_value::<Pattern>(json).unwrap(), pattern);
        }
        for dropped in ["gray-64", "response", "colors", "Red"] {
            assert!(serde_json::from_value::<Pattern>(dropped.into()).is_err());
        }
    }

    #[test]
    fn calibration_disables_smoothing_and_optionally_correction() {
        let configured = Config {
            correction: ColorCorrection::new(40, 0, 0).unwrap(),
            brightness_percent: BrightnessPercent(55),
            smoothing_seconds: SmoothingSeconds(2.5),
            remove_black_bars: true,
            ..Config::default()
        };
        let corrected = Calibration {
            pattern: Pattern::Gray192,
            uncorrected: false,
        }
        .adjust(configured);
        assert_eq!(
            corrected,
            Config {
                smoothing_seconds: SmoothingSeconds(0.0),
                ..configured
            }
        );
        let uncorrected = Calibration {
            pattern: Pattern::Gray192,
            uncorrected: true,
        }
        .adjust(configured);
        assert_eq!(
            uncorrected,
            Config {
                smoothing_seconds: SmoothingSeconds(0.0),
                correction: ColorCorrection::default(),
                ..configured
            }
        );
        assert!(
            serde_json::from_value::<Calibration>(
                serde_json::json!({ "pattern": "red", "uncorrected": false, "x": 1 })
            )
            .is_err()
        );
    }
}
