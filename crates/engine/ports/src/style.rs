//! Styles: one choice that sets the picture-taste knobs. The built-in styles'
//! values live only in [`Style::built_in`], so a retuned style reaches every
//! settings file and frontend with the next build; the file stores the name.
use crate::ValidationError;
use prysm_core::DarkFloorPercent;
use serde::{Deserialize, Serialize};

/// A style by name. [`Processing::style`](crate::Processing::style) stores it
/// as an object, `{ "kind": "natural" }`; on its own it is written as its
/// name, as a key of the published table of built-in styles.
///
/// Built-in and custom are one enum because they are one flat `kind` on the
/// wire; [`Style::built_in`] has no values for Custom.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    /// Calm in quiet scenes, follows cuts at once.
    #[default]
    Natural,
    /// A slow, soft glow; nothing sudden.
    Calm,
    /// Stronger colours, follows the action.
    Vivid,
    /// Reacts at once, with the least delay.
    Game,
    /// The user's own values, in `Processing::custom_style`.
    Custom,
}

impl Style {
    /// The built-in styles, in the order they are offered.
    pub const BUILT_IN: [Self; 4] = [Self::Natural, Self::Calm, Self::Vivid, Self::Game];

    /// The built-in styles' values, the one table of them; `None` for
    /// [`Style::Custom`], whose values are the user's. First guesses, not yet
    /// tuned on a TV.
    #[must_use]
    pub const fn built_in(self) -> Option<StyleValues> {
        let values = match self {
            Self::Natural => StyleValues {
                smoothing_seconds: 0.3,
                cut_response_percent: 100,
                brightness_ramp_seconds: 0.0,
                spatial_reach_percent: 10,
                color_focus_percent: 0,
                saturation_boost_percent: 0,
                dark_floor_percent: 0,
            },
            // Cuts are eased in and brightness is ramped: nothing abrupt.
            Self::Calm => StyleValues {
                smoothing_seconds: 0.8,
                cut_response_percent: 0,
                brightness_ramp_seconds: 1.0,
                spatial_reach_percent: 20,
                color_focus_percent: 0,
                saturation_boost_percent: 0,
                dark_floor_percent: 0,
            },
            Self::Vivid => StyleValues {
                smoothing_seconds: 0.15,
                cut_response_percent: 100,
                brightness_ramp_seconds: 0.0,
                spatial_reach_percent: 8,
                color_focus_percent: 0,
                saturation_boost_percent: 30,
                dark_floor_percent: 0,
            },
            Self::Game => StyleValues {
                smoothing_seconds: 0.05,
                cut_response_percent: 100,
                brightness_ramp_seconds: 0.0,
                spatial_reach_percent: 6,
                color_focus_percent: 0,
                saturation_boost_percent: 0,
                dark_floor_percent: 0,
            },
            Self::Custom => return None,
        };
        Some(values)
    }
}

/// [`Processing::style`](crate::Processing::style) on the wire:
/// `{ "kind": "natural" }`, an object so that a later kind with fields (a
/// saved style by `id`) is additive. A serde `tag` would not refuse unknown
/// fields beside the kind.
pub(crate) mod as_object {
    use super::Style;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    #[serde(remote = "Self", deny_unknown_fields)]
    struct Object {
        kind: Style,
    }
    crate::map_only!(Object, r#"a style such as { "kind": "natural" }"#);

    pub fn serialize<S: Serializer>(style: &Style, serializer: S) -> Result<S::Ok, S::Error> {
        <Object as Serialize>::serialize(&Object { kind: *style }, serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Style, D::Error> {
        <Object as Deserialize>::deserialize(deserializer).map(|object| object.kind)
    }
}

/// The picture-taste knobs a style sets, in plain units: a built-in style's
/// row, or the user's own in `Processing::custom_style`. Each field is
/// required, so a custom style says every value it applies.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", deny_unknown_fields)]
pub struct StyleValues {
    /// Seconds to complete most of a change, at most
    /// [`StyleValues::MAX_SMOOTHING_SECONDS`]; 0 disables smoothing.
    pub smoothing_seconds: f32,
    /// How much of a scene cut the lights take at once.
    pub cut_response_percent: u8,
    /// At most [`StyleValues::MAX_BRIGHTNESS_RAMP_SECONDS`]; 0 sets no limit.
    pub brightness_ramp_seconds: f32,
    /// Edge depth: how far into the picture each light looks.
    pub spatial_reach_percent: u8,
    pub color_focus_percent: u8,
    pub saturation_boost_percent: u8,
    /// At most [`StyleValues::MAX_DARK_FLOOR_PERCENT`].
    pub dark_floor_percent: u8,
}
crate::map_only!(StyleValues, "style values as an object");

impl StyleValues {
    /// The default style's values, which a custom style without values (an
    /// invalid setting) falls back to. Evaluated when compiled, so it cannot
    /// fail at run time.
    pub const DEFAULT: Self = Style::Natural.built_in().unwrap();

    /// The longest smoothing: slower than this, the lights trail the
    /// picture by more than a scene lasts.
    pub const MAX_SMOOTHING_SECONDS: f32 = 2.0;

    /// The longest brightness ramp: slower than this, a cut to a bright
    /// scene looks like a fade rather than a cut.
    pub const MAX_BRIGHTNESS_RAMP_SECONDS: f32 = 2.0;

    /// The brightest dark-scene glow, in percent.
    pub const MAX_DARK_FLOOR_PERCENT: u8 = DarkFloorPercent::MAX;

    /// Rejects values the processor would silently clamp or ignore, naming
    /// the field under `at`, such as `processing.custom_style`.
    ///
    /// # Errors
    /// The first value out of range.
    pub fn validate(&self, at: &str) -> Result<(), ValidationError> {
        let path = |field: &str| format!("{at}.{field}");
        let percent = |field: &str, value: u8, min: u8, max: u8| {
            (min..=max)
                .contains(&value)
                .then_some(())
                .ok_or_else(|| ValidationError::new(path(field), format!("must be {min}-{max}")))
        };
        if !(0.0..=Self::MAX_SMOOTHING_SECONDS).contains(&self.smoothing_seconds) {
            return Err(ValidationError::new(
                path("smoothing_seconds"),
                format!(
                    "must be 0 to {} seconds, 0 to disable",
                    Self::MAX_SMOOTHING_SECONDS
                ),
            ));
        }
        percent("cut_response_percent", self.cut_response_percent, 0, 100)?;
        if !(0.0..=Self::MAX_BRIGHTNESS_RAMP_SECONDS).contains(&self.brightness_ramp_seconds) {
            return Err(ValidationError::new(
                path("brightness_ramp_seconds"),
                format!(
                    "must be 0 to {} seconds, 0 for no limit",
                    Self::MAX_BRIGHTNESS_RAMP_SECONDS
                ),
            ));
        }
        percent("spatial_reach_percent", self.spatial_reach_percent, 1, 100)?;
        percent("color_focus_percent", self.color_focus_percent, 0, 100)?;
        percent(
            "saturation_boost_percent",
            self.saturation_boost_percent,
            0,
            100,
        )?;
        percent(
            "dark_floor_percent",
            self.dark_floor_percent,
            0,
            Self::MAX_DARK_FLOOR_PERCENT,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_style_has_valid_values_and_custom_has_none() {
        for style in Style::BUILT_IN {
            let values = style.built_in().unwrap();
            values.validate("style").unwrap();
        }
        assert_eq!(Style::Custom.built_in(), None);
        assert_eq!(Style::default().built_in(), Some(StyleValues::DEFAULT));
    }

    /// Every value is required, but serde's derive alone would read them by
    /// position from an array.
    #[test]
    fn only_an_object_is_style_values() {
        let natural = serde_json::to_value(StyleValues::DEFAULT).unwrap();
        assert_eq!(
            serde_json::from_value::<StyleValues>(natural).unwrap(),
            StyleValues::DEFAULT
        );
        let error =
            serde_json::from_value::<StyleValues>(serde_json::json!([0.5, 50, 0.3, 50, 50, 50, 5]))
                .unwrap_err()
                .to_string();
        assert!(error.contains("style values as an object"), "{error}");
    }

    #[test]
    fn a_style_is_written_by_name() {
        for (style, name) in [
            (Style::Natural, "natural"),
            (Style::Calm, "calm"),
            (Style::Vivid, "vivid"),
            (Style::Game, "game"),
            (Style::Custom, "custom"),
        ] {
            let written = serde_json::to_value(style).unwrap();
            assert_eq!(written, name);
            assert_eq!(serde_json::from_value::<Style>(written).unwrap(), style);
        }
    }

    #[test]
    fn out_of_range_values_are_refused_at_their_path() {
        let natural = Style::Natural.built_in().unwrap();
        let at = "processing.custom_style";
        for (values, field) in [
            (
                StyleValues {
                    smoothing_seconds: f32::NAN,
                    ..natural
                },
                "smoothing_seconds",
            ),
            (
                StyleValues {
                    cut_response_percent: 101,
                    ..natural
                },
                "cut_response_percent",
            ),
            (
                StyleValues {
                    brightness_ramp_seconds: 2.01,
                    ..natural
                },
                "brightness_ramp_seconds",
            ),
            (
                StyleValues {
                    spatial_reach_percent: 0,
                    ..natural
                },
                "spatial_reach_percent",
            ),
            (
                StyleValues {
                    color_focus_percent: 101,
                    ..natural
                },
                "color_focus_percent",
            ),
            (
                StyleValues {
                    saturation_boost_percent: 101,
                    ..natural
                },
                "saturation_boost_percent",
            ),
            (
                StyleValues {
                    dark_floor_percent: 21,
                    ..natural
                },
                "dark_floor_percent",
            ),
        ] {
            assert_eq!(
                values.validate(at).unwrap_err().field,
                format!("{at}.{field}")
            );
        }
        for seconds in [-0.1, 2.01, f32::INFINITY] {
            let values = StyleValues {
                brightness_ramp_seconds: seconds,
                ..natural
            };
            assert!(values.validate(at).is_err(), "{seconds}");
            let values = StyleValues {
                smoothing_seconds: seconds,
                ..natural
            };
            assert!(values.validate(at).is_err(), "{seconds}");
        }
        let longest = StyleValues {
            smoothing_seconds: StyleValues::MAX_SMOOTHING_SECONDS,
            ..natural
        };
        assert_eq!(longest.validate(at), Ok(()));
        let floor = StyleValues {
            dark_floor_percent: 21,
            ..natural
        };
        assert_eq!(floor.validate(at).unwrap_err().reason, "must be 0-20");
        StyleValues {
            saturation_boost_percent: 100,
            dark_floor_percent: 20,
            brightness_ramp_seconds: 2.0,
            ..natural
        }
        .validate(at)
        .unwrap();
    }
}
