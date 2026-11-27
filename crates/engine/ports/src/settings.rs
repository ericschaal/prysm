use crate::ValidationError;
use crate::calibrations::{self, Calibration};
use crate::style::{self, Style, StyleValues};
use prysm_core::{
    BrightnessPercent, BrightnessRampSeconds, ColorCorrection, ColorFocusPercent, Config,
    CutResponsePercent, DarkFloorPercent, SampleDensity, SaturationBoostPercent, SmoothingSeconds,
    SpatialReachPercent,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;

/// Everything a user can change at runtime. Serialized as the fields of a
/// human-editable file (`prysm-settings-file` adds the file's `format`):
///
/// ```json
/// {
///   "capture": { "kind": "device", "device": "/dev/video0" },
///   "output": { "kind": "wled", "address": "wled.local", "leds": { "top": 96 } },
///   "processing": {
///     "brightness_percent": 80,
///     "style": { "kind": "calm" },
///     "calibrations": [
///       { "id": "ps5", "name": "PS5", "correction": { "warmth": 12, "tint": -4 } }
///     ],
///     "active_calibration": "ps5"
///   }
/// }
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", deny_unknown_fields)]
pub struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<AdapterSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<AdapterSettings>,
    #[serde(default)]
    pub processing: Processing,
}
crate::map_only!(Settings, "settings as an object");

/// A top-level [`Settings`] section, named in validation field paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Capture,
    Output,
    Processing,
}

impl Section {
    /// The [`Settings`] field name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Capture => "capture",
            Self::Output => "output",
            Self::Processing => "processing",
        }
    }
}

impl fmt::Display for Section {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One adapter's settings: `{ "kind": "...", ...fields }`. The fields stay
/// untyped here; the factory registered for `kind` decodes and validates them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdapterSettings {
    pub kind: String,
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

impl AdapterSettings {
    /// Encode typed adapter settings under `kind`.
    ///
    /// # Panics
    /// If `settings` does not serialize to a JSON object, which is a bug in the adapter.
    pub fn new(kind: impl Into<String>, settings: &impl Serialize) -> Self {
        match serde_json::to_value(settings) {
            Ok(Value::Object(fields)) => Self {
                kind: kind.into(),
                fields,
            },
            _ => panic!("adapter settings must serialize to a JSON object"),
        }
    }
}

/// Processing controls in plain units: brightness, the chosen style and the
/// user's own style values, detail, black bars, and the saved calibrations
/// with the active one. Converted to [`Config`] for the processor, which
/// never learns about styles. Any change here, including to calibrations, is
/// applied to a running session at the next frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", default, deny_unknown_fields)]
pub struct Processing {
    pub brightness_percent: u8,
    #[serde(with = "style::as_object")]
    pub style: Style,
    /// The user's own values, applied while [`Style::Custom`] is chosen and
    /// kept while another style is; absent until first tuned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_style: Option<StyleValues>,
    /// Samples per 1000 px of edge, measured on the picture scaled down to
    /// 640x360, 1 to [`Processing::MAX_SAMPLE_DENSITY`]: the same value gives
    /// the same samples at any capture size. At most one sample per
    /// sampling-grid cell along an edge (128 along the longer side): density
    /// 200 reaches it for 16:9 pictures of 640x360 or more, a higher one for
    /// squarer or smaller pictures.
    pub sample_density: u16,
    pub remove_black_bars: bool,
    /// At most [`MAX_CALIBRATIONS`](crate::MAX_CALIBRATIONS), with unique ids.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub calibrations: Vec<Calibration>,
    /// The `id` of the calibration whose correction is applied; none applies no correction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_calibration: Option<String>,
}

crate::map_only!(Processing, "processing settings as an object");

impl Default for Processing {
    fn default() -> Self {
        let config = Config::default();
        Self {
            brightness_percent: config.brightness_percent.0,
            style: Style::default(),
            custom_style: None,
            sample_density: u16::try_from(config.sample_density.0).unwrap_or(u16::MAX),
            remove_black_bars: config.remove_black_bars,
            calibrations: Vec::new(),
            active_calibration: None,
        }
    }
}

impl Processing {
    /// The most a user may set, in the file as on the portal's slider: above
    /// every density that still adds samples. The sampler gives an edge at
    /// most one sample per grid cell, which density 128,000 / (the 640x360
    /// copy's longer side) reaches: 200 for 16:9, about 267 for a square, 400
    /// for a 320x180 picture.
    pub const MAX_SAMPLE_DENSITY: u16 = 512;

    /// # Errors
    /// Rejects values the processor would silently clamp or ignore, and a
    /// custom style without values.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let path = |field: &str| format!("{}.{field}", Section::Processing);
        if self.brightness_percent > 100 {
            return Err(ValidationError::new(
                path("brightness_percent"),
                "must be 0-100",
            ));
        }
        match (self.style, &self.custom_style) {
            (Style::Custom, None) => {
                return Err(ValidationError::new(
                    path("custom_style"),
                    "must be set when the style is custom",
                ));
            }
            // Kept values are checked whichever style is chosen: they are
            // what Custom applies when it is chosen again.
            (_, Some(values)) => values.validate(&path("custom_style"))?,
            (_, None) => {}
        }
        if !(1..=Self::MAX_SAMPLE_DENSITY).contains(&self.sample_density) {
            return Err(ValidationError::new(
                path("sample_density"),
                format!("must be 1-{}", Self::MAX_SAMPLE_DENSITY),
            ));
        }
        calibrations::validate(
            Section::Processing,
            &self.calibrations,
            self.active_calibration.as_deref(),
        )
    }

    /// The chosen style's values. A custom style without values, which
    /// [`Processing::validate`] refuses, uses the default style's.
    #[must_use]
    pub fn style_values(&self) -> StyleValues {
        match self.style {
            Style::Custom => self.custom_style,
            built_in => built_in.built_in(),
        }
        .unwrap_or(StyleValues::DEFAULT)
    }

    /// The active calibration, if one is set and exists.
    #[must_use]
    pub fn active(&self) -> Option<&Calibration> {
        let active = self.active_calibration.as_deref()?;
        self.calibrations
            .iter()
            .find(|calibration| calibration.id == active)
    }

    /// The active calibration's correction. No correction when none is active, or
    /// when loaded settings name a missing or out-of-range calibration (which
    /// [`Processing::validate`] reports).
    #[must_use]
    pub fn correction(&self) -> ColorCorrection {
        self.active()
            .and_then(|calibration| calibration.correction.to_core())
            .unwrap_or_default()
    }

    /// The processor's config, with the chosen style resolved to its values.
    #[must_use]
    pub fn config(&self) -> Config {
        let style = self.style_values();
        Config {
            correction: self.correction(),
            brightness_percent: BrightnessPercent(self.brightness_percent),
            saturation_boost_percent: SaturationBoostPercent(style.saturation_boost_percent),
            dark_floor_percent: DarkFloorPercent(style.dark_floor_percent),
            smoothing_seconds: SmoothingSeconds(style.smoothing_seconds),
            cut_response_percent: CutResponsePercent(style.cut_response_percent),
            brightness_ramp_seconds: BrightnessRampSeconds(style.brightness_ramp_seconds),
            sample_density: SampleDensity(usize::from(self.sample_density)),
            spatial_reach_percent: SpatialReachPercent(style.spatial_reach_percent),
            color_focus_percent: ColorFocusPercent(style.color_focus_percent),
            remove_black_bars: self.remove_black_bars,
        }
    }
}

/// Optimistic-concurrency token for settings. It starts at a random value in
/// each process and increases by one per accepted replacement, so a revision
/// read before a restart is stale afterwards. It is not persisted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Revision(pub u64);

impl Revision {
    #[must_use]
    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_core::PrimaryAdjustment;

    #[test]
    fn settings_file_is_kind_tagged_and_defaults_processing() {
        let settings: Settings =
            serde_json::from_str(r#"{ "capture": { "kind": "device", "device": "/dev/video0" } }"#)
                .unwrap();
        let capture = settings.capture.as_ref().unwrap();
        assert_eq!(capture.kind, "device");
        assert_eq!(capture.fields["device"], "/dev/video0");
        assert_eq!(settings.processing, Processing::default());

        let round = serde_json::to_value(&settings).unwrap();
        assert_eq!(round["capture"]["kind"], "device");
        assert!(round.get("output").is_none());
    }

    #[test]
    fn section_names_are_settings_fields() {
        let adapter = AdapterSettings::new("any", &serde_json::json!({}));
        let settings = Settings {
            capture: Some(adapter.clone()),
            output: Some(adapter),
            processing: Processing::default(),
        };
        let written = serde_json::to_value(&settings).unwrap();
        for section in [Section::Capture, Section::Output, Section::Processing] {
            assert!(written.get(section.as_str()).is_some(), "{section}");
        }
    }

    /// The style knobs of `config`, to compare with a style's values.
    fn knobs(config: Config) -> StyleValues {
        StyleValues {
            smoothing_seconds: config.smoothing_seconds.0,
            cut_response_percent: config.cut_response_percent.0,
            brightness_ramp_seconds: config.brightness_ramp_seconds.0,
            spatial_reach_percent: config.spatial_reach_percent.0,
            color_focus_percent: config.color_focus_percent.0,
            saturation_boost_percent: config.saturation_boost_percent.0,
            dark_floor_percent: config.dark_floor_percent.0,
        }
    }

    /// Values unlike every built-in style's.
    const MINE: StyleValues = StyleValues {
        smoothing_seconds: 0.5,
        cut_response_percent: 40,
        brightness_ramp_seconds: 0.5,
        spatial_reach_percent: 30,
        color_focus_percent: 60,
        saturation_boost_percent: 40,
        dark_floor_percent: 10,
    };

    #[test]
    fn processing_rejects_out_of_range_values() {
        let invalid = Processing {
            brightness_percent: 101,
            ..Processing::default()
        };
        assert_eq!(
            invalid.validate().unwrap_err().field,
            "processing.brightness_percent"
        );
        for sample_density in [0, Processing::MAX_SAMPLE_DENSITY + 1, u16::MAX] {
            let invalid = Processing {
                sample_density,
                ..Processing::default()
            };
            assert_eq!(
                invalid.validate().unwrap_err().field,
                "processing.sample_density",
                "{sample_density}"
            );
        }
        let most = Processing {
            sample_density: Processing::MAX_SAMPLE_DENSITY,
            ..Processing::default()
        };
        most.validate().unwrap();
    }

    #[test]
    fn each_built_in_style_sets_its_values_in_the_config() {
        for style in Style::BUILT_IN {
            let processing = Processing {
                style,
                custom_style: Some(MINE),
                ..Processing::default()
            };
            processing.validate().unwrap();
            let config = processing.config();
            assert_eq!(Some(knobs(config)), style.built_in(), "{style:?}");
            assert_eq!(
                config.brightness_percent,
                Config::default().brightness_percent
            );
            assert_eq!(config.sample_density, Config::default().sample_density);
        }
        let natural = Processing::default().config();
        assert_eq!(Some(knobs(natural)), Style::Natural.built_in());
        assert_eq!(natural.cut_response_percent, CutResponsePercent(100));
        assert_eq!(natural.smoothing_seconds, SmoothingSeconds(0.3));
    }

    #[test]
    fn custom_sets_the_users_values_and_needs_them() {
        let custom = Processing {
            style: Style::Custom,
            custom_style: Some(MINE),
            ..Processing::default()
        };
        custom.validate().unwrap();
        assert_eq!(knobs(custom.config()), MINE);

        let missing = Processing {
            style: Style::Custom,
            ..Processing::default()
        };
        let error = missing.validate().unwrap_err();
        assert_eq!(error.field, "processing.custom_style");
        assert_eq!(error.reason, "must be set when the style is custom");
        assert_eq!(knobs(missing.config()), StyleValues::DEFAULT);
    }

    #[test]
    fn out_of_range_custom_values_are_refused_at_their_path_whichever_style_is_chosen() {
        for style in [Style::Custom, Style::Calm] {
            let invalid = Processing {
                style,
                custom_style: Some(StyleValues {
                    cut_response_percent: 101,
                    ..MINE
                }),
                ..Processing::default()
            };
            assert_eq!(
                invalid.validate().unwrap_err().field,
                "processing.custom_style.cut_response_percent"
            );
        }
        let invalid = Processing {
            style: Style::Custom,
            custom_style: Some(StyleValues {
                dark_floor_percent: 21,
                ..MINE
            }),
            ..Processing::default()
        };
        let error = invalid.validate().unwrap_err();
        assert_eq!(error.field, "processing.custom_style.dark_floor_percent");
        assert_eq!(error.reason, "must be 0-20");
    }

    #[test]
    fn the_style_is_stored_by_name_and_custom_values_are_kept() {
        let omitted: Processing =
            serde_json::from_value(serde_json::json!({ "brightness_percent": 80 })).unwrap();
        assert_eq!(omitted.style, Style::Natural);
        let written = serde_json::to_value(&omitted).unwrap();
        assert_eq!(written["style"], serde_json::json!({ "kind": "natural" }));
        assert!(written.get("custom_style").is_none(), "{written}");

        let calm: Processing = serde_json::from_value(serde_json::json!({
            "style": { "kind": "calm" },
            "custom_style": serde_json::to_value(MINE).unwrap(),
        }))
        .unwrap();
        calm.validate().unwrap();
        assert_eq!((calm.style, calm.custom_style), (Style::Calm, Some(MINE)));
        let written = serde_json::to_value(&calm).unwrap();
        assert_eq!(written["custom_style"]["color_focus_percent"], 60);
        assert_eq!(serde_json::from_value::<Processing>(written).unwrap(), calm);

        for (style, refusal) in [
            (
                serde_json::json!({ "kind": "cinema" }),
                "unknown variant `cinema`",
            ),
            (
                serde_json::json!({ "kind": "calm", "id": "x" }),
                "unknown field `id`",
            ),
            (
                serde_json::json!("calm"),
                r#"expected a style such as { "kind": "natural" }"#,
            ),
            (serde_json::json!({}), "missing field `kind`"),
        ] {
            let error = serde_json::from_value::<Processing>(serde_json::json!({ "style": style }))
                .unwrap_err()
                .to_string();
            assert!(error.contains(refusal), "{error}");
        }
        assert!(
            serde_json::from_value::<Processing>(serde_json::json!({
                "style": { "kind": "custom" },
                "custom_style": { "smoothing_seconds": 0.2 },
            }))
            .is_err()
        );
        for removed in [
            "smoothing_seconds",
            "cut_response_percent",
            "brightness_ramp_seconds",
            "spatial_reach_percent",
            "color_focus_percent",
            "saturation_boost_percent",
            "dark_floor_percent",
        ] {
            let text = format!(r#"{{ "{removed}": 0 }}"#);
            assert!(
                serde_json::from_str::<Processing>(&text).is_err(),
                "{removed}"
            );
        }
    }

    /// The chosen style, the custom values and a calibration are objects:
    /// an array is refused at its path, not read by position.
    #[test]
    fn an_array_for_a_style_or_calibration_is_refused_at_its_path() {
        for (processing, path) in [
            (
                serde_json::json!({ "custom_style": [0.5, 50, 0.3, 50, 50, 50, 5] }),
                "custom_style",
            ),
            (serde_json::json!({ "style": ["natural"] }), "style"),
            (
                serde_json::json!({ "calibrations": [["a", "A"]] }),
                "calibrations[0]",
            ),
        ] {
            let error =
                serde_path_to_error::deserialize::<_, Processing>(processing.clone()).unwrap_err();
            assert_eq!(error.path().to_string(), path, "{processing}");
            assert!(
                error.inner().to_string().contains("invalid type: sequence"),
                "{processing}: {error}"
            );
        }
    }

    #[test]
    fn active_calibration_sets_the_processor_correction() {
        let processing: Processing = serde_json::from_value(serde_json::json!({
            "calibrations": [
                { "id": "ps5", "name": "PS5", "correction": { "warmth": 40, "red": { "hue": -5 } } },
                { "id": "apps", "name": "TV apps" }
            ],
            "active_calibration": "ps5"
        }))
        .unwrap();
        processing.validate().unwrap();
        let expected = ColorCorrection::new(
            40,
            0,
            0,
            [
                PrimaryAdjustment {
                    hue: -5,
                    strength: 0,
                },
                PrimaryAdjustment::default(),
                PrimaryAdjustment::default(),
            ],
        )
        .unwrap();
        assert_eq!(processing.config().correction, expected);
        assert_eq!(
            Processing {
                active_calibration: Some("apps".into()),
                ..processing.clone()
            }
            .config()
            .correction,
            ColorCorrection::default()
        );
        let missing = Processing {
            active_calibration: Some("gone".into()),
            ..processing.clone()
        };
        assert_eq!(
            missing.validate().unwrap_err().field,
            "processing.active_calibration"
        );
        assert_eq!(missing.config().correction, ColorCorrection::default());

        let written = serde_json::to_value(&processing).unwrap();
        assert_eq!(
            written["calibrations"][1]["correction"]["blue"]["strength"],
            0
        );
        assert_eq!(
            serde_json::from_value::<Processing>(written).unwrap(),
            processing
        );
        let empty = serde_json::to_value(Processing::default()).unwrap();
        assert!(empty.get("calibrations").is_none() && empty.get("active_calibration").is_none());
    }
}
