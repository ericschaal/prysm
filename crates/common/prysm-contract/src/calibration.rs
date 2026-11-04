//! Persisted color profiles and the temporary calibration protocol.
use crate::{ColorAdjustment, OperationId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ColorCorrection {
    pub warmth: ColorAdjustment,
    pub tint: ColorAdjustment,
    pub midtones: ColorAdjustment,
    #[serde(default, skip_serializing_if = "neutral")]
    pub intensity: ColorAdjustment,
    /// Red, yellow, green, cyan, blue, magenta in wheel order.
    #[serde(default, skip_serializing_if = "neutral")]
    #[schema(min_items = 6, max_items = 6)]
    pub hue: [ColorAdjustment; 6],
}
fn neutral<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}
impl ColorCorrection {
    #[must_use]
    #[allow(
        clippy::missing_panics_doc,
        reason = "bounded controls guarantee core validation"
    )]
    pub fn to_core(self) -> prysm_core::ColorCorrection {
        prysm_core::ColorCorrection::new(self.warmth.get(), self.tint.get(), self.midtones.get())
            .and_then(|c| c.with_chroma(self.intensity.get(), self.hue.map(ColorAdjustment::get)))
            .expect("validated correction controls")
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CalibrationProfile {
    pub id: OperationId,
    pub name: String,
    pub correction: ColorCorrection,
}
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CalibrationProfiles {
    pub profiles: Vec<CalibrationProfile>,
    pub active: Option<OperationId>,
}
impl CalibrationProfiles {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.profiles.len() <= 32,
            "at most 32 calibration profiles are allowed"
        );
        for (index, profile) in self.profiles.iter().enumerate() {
            validate_name(&profile.name)?;
            anyhow::ensure!(
                !self.profiles[..index].iter().any(|p| p.id == profile.id),
                "duplicate calibration profile id"
            );
        }
        anyhow::ensure!(
            self.active
                .is_none_or(|id| self.profiles.iter().any(|p| p.id == id)),
            "active calibration profile does not exist"
        );
        Ok(())
    }
    #[must_use]
    pub fn correction(&self) -> ColorCorrection {
        self.profiles
            .iter()
            .find(|p| Some(p.id) == self.active)
            .map(|p| p.correction)
            .unwrap_or_default()
    }
}
pub fn validate_name(name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !name.trim().is_empty()
            && name == name.trim()
            && name.chars().count() <= 64
            && !name.chars().any(char::is_control),
        "profile name must contain 1–64 characters without surrounding whitespace or control characters"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_corrections_round_trip_without_new_fields_and_bad_anchors_are_rejected() {
        let old = serde_json::json!({"warmth":58,"tint":-31,"midtones":0});
        let parsed: ColorCorrection = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), old);
        assert_eq!(
            parsed.to_core(),
            prysm_core::ColorCorrection::new(58, -31, 0).unwrap()
        );
        for bad in [
            serde_json::json!([1, 2, 3]),
            serde_json::json!([0, 0, 0, 0, 0, 101]),
        ] {
            let mut invalid = old.clone();
            invalid["hue"] = bad;
            assert!(serde_json::from_value::<ColorCorrection>(invalid).is_err());
        }
    }
    #[test]
    fn profile_validation_and_identity_defaults() {
        let empty = CalibrationProfiles::default();
        empty.validate().unwrap();
        assert_eq!(empty.correction(), ColorCorrection::default());
        let id = OperationId::new();
        let mut profiles = CalibrationProfiles {
            profiles: vec![CalibrationProfile {
                id,
                name: "PS5".into(),
                correction: ColorCorrection::default(),
            }],
            active: Some(id),
        };
        profiles.validate().unwrap();
        profiles.profiles.clear();
        assert!(profiles.validate().is_err());
        profiles.active = None;
        profiles.validate().unwrap();
        for invalid in ["", " ", "PS5\n", " name"] {
            assert!(validate_name(invalid).is_err());
        }
        for invalid in [
            serde_json::json!(101),
            serde_json::json!(-101),
            serde_json::json!(0.5),
            serde_json::json!(null),
        ] {
            assert!(
                serde_json::from_value::<ColorCorrection>(
                    serde_json::json!({"warmth": invalid, "tint":0,"midtones":0})
                )
                .is_err()
            );
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CalibrationStep {
    #[default]
    Choose,
    Response,
    WhiteBalance,
    Grays,
    Intensity,
    Colors,
    Mixed,
    Review,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum CalibrationRequest {
    Begin,
    Pair {
        pairing: OperationId,
    },
    Poll {
        token: OperationId,
        compare: bool,
        playing: bool,
    },
    Select {
        token: OperationId,
        profile: Option<OperationId>,
    },
    Update {
        token: OperationId,
        sequence: u32,
        correction: ColorCorrection,
        step: CalibrationStep,
        shade: u8,
        paused: bool,
        compare: bool,
    },
    Save {
        token: OperationId,
        name: String,
    },
    Cancel {
        token: OperationId,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "wire snapshot contains independent playback, pairing and comparison flags"
)]
pub struct CalibrationView {
    pub saved: bool,
    pub token: OperationId,
    pub pairing: Option<OperationId>,
    pub connected: bool,
    pub draft: CalibrationProfile,
    pub step: CalibrationStep,
    pub shade: u8,
    pub paused: bool,
    pub compare: bool,
    pub sequence: u32,
    pub ended: Option<String>,
}
