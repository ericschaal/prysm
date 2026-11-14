use crate::ValidationError;
use crate::profiles::{self, ColorProfile};
use prysm_core::{
    BrightnessPercent, BrightnessRampSeconds, ColorCorrection, ColorFocusPercent, Config,
    CutResponsePercent, SampleDensity, ShadowLiftPercent, SmoothingSeconds, SpatialReachPercent,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;

/// Everything a user can change at runtime. Serialized as a human-editable file:
///
/// ```json
/// {
///   "capture": { "kind": "device", "path": "/dev/video0" },
///   "output": { "kind": "wled", "address": "wled.local", "leds": { "top": 96 } },
///   "processing": {
///     "brightness_percent": 80,
///     "profiles": [
///       { "id": "ps5", "name": "PS5", "correction": { "warmth": 12, "tint": -4 } }
///     ],
///     "active_profile": "ps5"
///   }
/// }
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<AdapterSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<AdapterSettings>,
    #[serde(default)]
    pub processing: Processing,
}

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

/// Processing controls in plain units, plus the saved color profiles and the
/// active one; converted to [`Config`] for the processor. Any change here,
/// including to profiles, is applied to a running session at the next frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Processing {
    pub brightness_percent: u8,
    pub shadow_lift_percent: u8,
    pub smoothing_seconds: f32,
    pub cut_response_percent: u8,
    /// At most [`Processing::MAX_BRIGHTNESS_RAMP_SECONDS`]; 0 sets no limit.
    pub brightness_ramp_seconds: f32,
    pub sample_density: u16,
    pub spatial_reach_percent: u8,
    pub color_focus_percent: u8,
    pub remove_black_bars: bool,
    /// At most [`MAX_PROFILES`](crate::MAX_PROFILES), with unique ids.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<ColorProfile>,
    /// The `id` of the profile whose correction is applied; none applies no correction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_profile: Option<String>,
}

impl Default for Processing {
    fn default() -> Self {
        let config = Config::default();
        Self {
            brightness_percent: config.brightness_percent.0,
            shadow_lift_percent: config.shadow_lift_percent.0,
            smoothing_seconds: config.smoothing_seconds.0,
            cut_response_percent: config.cut_response_percent.0,
            brightness_ramp_seconds: config.brightness_ramp_seconds.0,
            sample_density: u16::try_from(config.sample_density.0).unwrap_or(u16::MAX),
            spatial_reach_percent: config.spatial_reach_percent.0,
            color_focus_percent: config.color_focus_percent.0,
            remove_black_bars: config.remove_black_bars,
            profiles: Vec::new(),
            active_profile: None,
        }
    }
}

impl Processing {
    /// The longest brightness ramp: slower than this, a cut to a bright
    /// scene looks like a fade rather than a cut.
    pub const MAX_BRIGHTNESS_RAMP_SECONDS: f32 = 2.0;

    /// # Errors
    /// Rejects values the processor would silently clamp or ignore.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let path = |field: &str| format!("{}.{field}", Section::Processing);
        let percent = |field: &str, value: u8, min: u8| {
            (min..=100)
                .contains(&value)
                .then_some(())
                .ok_or_else(|| ValidationError::new(path(field), format!("must be {min}-100")))
        };
        percent("brightness_percent", self.brightness_percent, 0)?;
        percent("shadow_lift_percent", self.shadow_lift_percent, 0)?;
        percent("cut_response_percent", self.cut_response_percent, 0)?;
        percent("spatial_reach_percent", self.spatial_reach_percent, 1)?;
        percent("color_focus_percent", self.color_focus_percent, 0)?;
        if !(self.smoothing_seconds.is_finite() && self.smoothing_seconds >= 0.0) {
            return Err(ValidationError::new(
                path("smoothing_seconds"),
                "must be a finite number of seconds, 0 to disable",
            ));
        }
        if !(0.0..=Self::MAX_BRIGHTNESS_RAMP_SECONDS).contains(&self.brightness_ramp_seconds) {
            return Err(ValidationError::new(
                path("brightness_ramp_seconds"),
                format!(
                    "must be 0 to {} seconds, 0 for no limit",
                    Self::MAX_BRIGHTNESS_RAMP_SECONDS
                ),
            ));
        }
        if self.sample_density == 0 {
            return Err(ValidationError::new(
                path("sample_density"),
                "must be at least 1",
            ));
        }
        profiles::validate(
            Section::Processing,
            &self.profiles,
            self.active_profile.as_deref(),
        )
    }

    /// The active profile, if one is set and exists.
    #[must_use]
    pub fn active(&self) -> Option<&ColorProfile> {
        let active = self.active_profile.as_deref()?;
        self.profiles.iter().find(|profile| profile.id == active)
    }

    /// The active profile's correction. No correction when none is active, or
    /// when loaded settings name a missing or out-of-range profile (which
    /// [`Processing::validate`] reports).
    #[must_use]
    pub fn correction(&self) -> ColorCorrection {
        self.active()
            .and_then(|profile| profile.correction.to_core())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn config(&self) -> Config {
        Config {
            correction: self.correction(),
            brightness_percent: BrightnessPercent(self.brightness_percent),
            shadow_lift_percent: ShadowLiftPercent(self.shadow_lift_percent),
            smoothing_seconds: SmoothingSeconds(self.smoothing_seconds),
            cut_response_percent: CutResponsePercent(self.cut_response_percent),
            brightness_ramp_seconds: BrightnessRampSeconds(self.brightness_ramp_seconds),
            sample_density: SampleDensity(usize::from(self.sample_density)),
            spatial_reach_percent: SpatialReachPercent(self.spatial_reach_percent),
            color_focus_percent: ColorFocusPercent(self.color_focus_percent),
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

    #[test]
    fn settings_file_is_kind_tagged_and_defaults_processing() {
        let settings: Settings =
            serde_json::from_str(r#"{ "capture": { "kind": "device", "path": "/dev/video0" } }"#)
                .unwrap();
        let capture = settings.capture.as_ref().unwrap();
        assert_eq!(capture.kind, "device");
        assert_eq!(capture.fields["path"], "/dev/video0");
        assert_eq!(settings.processing, Processing::default());
        assert_eq!(settings.processing.config(), Config::default());

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

    #[test]
    fn processing_rejects_out_of_range_values() {
        let invalid = Processing {
            spatial_reach_percent: 0,
            ..Processing::default()
        };
        assert_eq!(
            invalid.validate().unwrap_err().field,
            "processing.spatial_reach_percent"
        );
        let invalid = Processing {
            smoothing_seconds: f32::NAN,
            ..Processing::default()
        };
        assert!(invalid.validate().is_err());
        let invalid = Processing {
            cut_response_percent: 101,
            ..Processing::default()
        };
        assert_eq!(
            invalid.validate().unwrap_err().field,
            "processing.cut_response_percent"
        );
        let invalid = Processing {
            color_focus_percent: 101,
            ..Processing::default()
        };
        assert_eq!(
            invalid.validate().unwrap_err().field,
            "processing.color_focus_percent"
        );
        for seconds in [-0.1, 2.01, f32::NAN, f32::INFINITY] {
            let invalid = Processing {
                brightness_ramp_seconds: seconds,
                ..Processing::default()
            };
            assert_eq!(
                invalid.validate().unwrap_err().field,
                "processing.brightness_ramp_seconds",
                "{seconds}"
            );
        }
    }

    #[test]
    fn cut_response_and_brightness_ramp_round_trip_and_default_when_omitted() {
        let omitted: Processing =
            serde_json::from_value(serde_json::json!({ "smoothing_seconds": 0.5 })).unwrap();
        assert_eq!(omitted.cut_response_percent, 0);
        assert_eq!(omitted.brightness_ramp_seconds, 0.0);
        let config = omitted.config();
        assert_eq!(
            config.cut_response_percent,
            Config::default().cut_response_percent
        );
        assert_eq!(
            config.brightness_ramp_seconds,
            Config::default().brightness_ramp_seconds
        );

        for (percent, seconds) in [(0, 0.0), (60, 0.5), (100, 2.0)] {
            let present: Processing = serde_json::from_value(serde_json::json!({
                "cut_response_percent": percent,
                "brightness_ramp_seconds": seconds,
            }))
            .unwrap();
            present.validate().unwrap();
            let config = present.config();
            assert_eq!(config.cut_response_percent, CutResponsePercent(percent));
            assert_eq!(
                config.brightness_ramp_seconds,
                BrightnessRampSeconds(seconds)
            );
            let written = serde_json::to_value(&present).unwrap();
            assert_eq!(written["cut_response_percent"], percent);
            assert_eq!(written["brightness_ramp_seconds"], f64::from(seconds));
            assert_eq!(
                serde_json::from_value::<Processing>(written).unwrap(),
                present
            );
        }
        // Fields that no longer exist are refused rather than ignored.
        for removed in ["smoothing_response_percent", "brightness_smoothing"] {
            let text = format!(r#"{{ "{removed}": 0 }}"#);
            assert!(
                serde_json::from_str::<Processing>(&text).is_err(),
                "{removed}"
            );
        }
    }

    #[test]
    fn color_focus_round_trips_and_defaults_when_omitted() {
        let omitted: Processing =
            serde_json::from_value(serde_json::json!({ "spatial_reach_percent": 20 })).unwrap();
        assert_eq!(omitted.color_focus_percent, 0);
        assert_eq!(
            omitted.config().color_focus_percent,
            Config::default().color_focus_percent
        );
        for percent in [0, 50, 100] {
            let present: Processing =
                serde_json::from_value(serde_json::json!({ "color_focus_percent": percent }))
                    .unwrap();
            present.validate().unwrap();
            assert_eq!(
                present.config().color_focus_percent,
                ColorFocusPercent(percent)
            );
            let written = serde_json::to_value(&present).unwrap();
            assert_eq!(written["color_focus_percent"], percent);
            assert_eq!(
                serde_json::from_value::<Processing>(written).unwrap(),
                present
            );
        }
    }

    #[test]
    fn active_profile_sets_the_processor_correction() {
        let processing: Processing = serde_json::from_value(serde_json::json!({
            "profiles": [
                { "id": "ps5", "name": "PS5", "correction": { "warmth": 40, "hue": { "red": -5 } } },
                { "id": "apps", "name": "TV apps" }
            ],
            "active_profile": "ps5"
        }))
        .unwrap();
        processing.validate().unwrap();
        let expected = ColorCorrection::new(40, 0, 0)
            .and_then(|c| c.with_chroma(0, [-5, 0, 0, 0, 0, 0]))
            .unwrap();
        assert_eq!(processing.config().correction, expected);
        assert_eq!(
            Processing {
                active_profile: Some("apps".into()),
                ..processing.clone()
            }
            .config()
            .correction,
            ColorCorrection::default()
        );
        let missing = Processing {
            active_profile: Some("gone".into()),
            ..processing.clone()
        };
        assert_eq!(
            missing.validate().unwrap_err().field,
            "processing.active_profile"
        );
        assert_eq!(missing.config().correction, ColorCorrection::default());

        // Every control is written, so the file shows what can be edited.
        let written = serde_json::to_value(&processing).unwrap();
        assert_eq!(written["profiles"][1]["correction"]["hue"]["magenta"], 0);
        assert_eq!(
            serde_json::from_value::<Processing>(written).unwrap(),
            processing
        );
        // No profiles write nothing.
        let empty = serde_json::to_value(Processing::default()).unwrap();
        assert!(empty.get("profiles").is_none() && empty.get("active_profile").is_none());
    }
}
