use crate::ValidationError;
use prysm_core::{
    BrightnessPercent, Config, SampleDensity, ShadowLiftPercent, SmoothingSeconds,
    SpatialReachPercent,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;

/// Everything a user can change at runtime. Serialized as a human-editable file:
///
/// ```json
/// {
///   "capture": { "kind": "device", "path": "/dev/video0" },
///   "output": { "kind": "wled", "host": "10.0.0.5" },
///   "processing": { "brightness_percent": 80 }
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

/// Processing controls in plain units; converted to [`Config`] for the processor.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Processing {
    pub brightness_percent: u8,
    pub shadow_lift_percent: u8,
    pub smoothing_seconds: f32,
    pub sample_density: u16,
    pub spatial_reach_percent: u8,
    pub remove_black_bars: bool,
}

impl Default for Processing {
    fn default() -> Self {
        let config = Config::default();
        Self {
            brightness_percent: config.brightness_percent.0,
            shadow_lift_percent: config.shadow_lift_percent.0,
            smoothing_seconds: config.smoothing_seconds.0,
            sample_density: u16::try_from(config.sample_density.0).unwrap_or(u16::MAX),
            spatial_reach_percent: config.spatial_reach_percent.0,
            remove_black_bars: config.remove_black_bars,
        }
    }
}

impl Processing {
    /// # Errors
    /// Rejects values the processor would silently clamp or ignore.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let percent = |field: &str, value: u8, min: u8| {
            (min..=100).contains(&value).then_some(()).ok_or_else(|| {
                ValidationError::new(format!("processing.{field}"), format!("must be {min}-100"))
            })
        };
        percent("brightness_percent", self.brightness_percent, 0)?;
        percent("shadow_lift_percent", self.shadow_lift_percent, 0)?;
        percent("spatial_reach_percent", self.spatial_reach_percent, 1)?;
        if !(self.smoothing_seconds.is_finite() && self.smoothing_seconds >= 0.0) {
            return Err(ValidationError::new(
                "processing.smoothing_seconds",
                "must be a finite number of seconds, 0 to disable",
            ));
        }
        if self.sample_density == 0 {
            return Err(ValidationError::new(
                "processing.sample_density",
                "must be at least 1",
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn config(&self) -> Config {
        Config {
            brightness_percent: BrightnessPercent(self.brightness_percent),
            shadow_lift_percent: ShadowLiftPercent(self.shadow_lift_percent),
            smoothing_seconds: SmoothingSeconds(self.smoothing_seconds),
            sample_density: SampleDensity(usize::from(self.sample_density)),
            spatial_reach_percent: SpatialReachPercent(self.spatial_reach_percent),
            remove_black_bars: self.remove_black_bars,
            ..Config::default()
        }
    }
}

/// Settings version for optimistic replacement. Counts accepted replacements
/// since the process started; it is not persisted.
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
    }
}
