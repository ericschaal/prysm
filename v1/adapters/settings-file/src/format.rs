//! Private disk representation. Domain types acquire no Serde or file-format dependencies.
use prysm_contract::{
    ControlError,
    application::Settings,
    calibration::{CalibrationProfile, ProfileId},
};
use prysm_core::{self as core, ColorCorrection, Config};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::num::NonZeroU64;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document<C, O> {
    capture: Option<C>,
    output: Option<O>,
    #[serde(default)]
    processing: Processing,
    #[serde(default)]
    profiles: Vec<Profile>,
    active_profile: Option<NonZeroU64>,
}

#[derive(Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Processing {
    correction: Correction,
    brightness_percent: u8,
    shadow_lift_percent: u8,
    smoothing_seconds: f32,
    sample_density: usize,
    spatial_reach_percent: u8,
    remove_black_bars: bool,
}
impl Default for Processing {
    fn default() -> Self {
        Config::default().into()
    }
}
impl From<Config> for Processing {
    fn from(c: Config) -> Self {
        Self {
            correction: c.correction.into(),
            brightness_percent: c.brightness_percent.0,
            shadow_lift_percent: c.shadow_lift_percent.0,
            smoothing_seconds: c.smoothing_seconds.0,
            sample_density: c.sample_density.0,
            spatial_reach_percent: c.spatial_reach_percent.0,
            remove_black_bars: c.remove_black_bars,
        }
    }
}
impl TryFrom<Processing> for Config {
    type Error = ControlError;
    fn try_from(c: Processing) -> Result<Self, Self::Error> {
        Ok(Self {
            correction: c.correction.try_into()?,
            brightness_percent: core::BrightnessPercent(c.brightness_percent),
            shadow_lift_percent: core::ShadowLiftPercent(c.shadow_lift_percent),
            smoothing_seconds: core::SmoothingSeconds(c.smoothing_seconds),
            sample_density: core::SampleDensity(c.sample_density),
            spatial_reach_percent: core::SpatialReachPercent(c.spatial_reach_percent),
            remove_black_bars: c.remove_black_bars,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Correction {
    gains: [f32; 3],
    tone: f32,
    intensity: i16,
    hue: [i16; 6],
}
impl From<ColorCorrection> for Correction {
    fn from(c: ColorCorrection) -> Self {
        Self {
            gains: c.gains(),
            tone: c.tone(),
            intensity: c.intensity(),
            hue: c.hue(),
        }
    }
}
impl TryFrom<Correction> for ColorCorrection {
    type Error = ControlError;
    fn try_from(c: Correction) -> Result<Self, Self::Error> {
        Self::from_coefficients(c.gains, c.tone)
            .and_then(|value| value.with_chroma(c.intensity, c.hue))
            .map_err(|reason| ControlError::Invalid {
                field: "correction",
                reason: reason.into(),
            })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    id: NonZeroU64,
    name: String,
    correction: Correction,
}

pub(super) fn encode<C: Serialize, O: Serialize>(
    settings: Settings<C, O>,
) -> Result<Vec<u8>, ControlError> {
    settings.validate()?;
    let document = Document {
        capture: settings.capture,
        output: settings.output,
        processing: settings.processing.into(),
        profiles: settings
            .profiles
            .into_iter()
            .map(|p| Profile {
                id: p.id.0,
                name: p.name,
                correction: p.correction.into(),
            })
            .collect(),
        active_profile: settings.active_profile.map(|id| id.0),
    };
    serde_json::to_vec_pretty(&document).map_err(|e| ControlError::Adapter(Box::new(e)))
}

pub(super) fn decode<C: DeserializeOwned, O: DeserializeOwned>(
    bytes: &[u8],
) -> Result<Settings<C, O>, ControlError> {
    let document: Document<C, O> =
        serde_json::from_slice(bytes).map_err(|error| ControlError::Invalid {
            field: "settings file",
            reason: error.to_string(),
        })?;
    let settings = Settings {
        capture: document.capture,
        output: document.output,
        processing: document.processing.try_into()?,
        profiles: document
            .profiles
            .into_iter()
            .map(|p| {
                Ok(CalibrationProfile {
                    id: ProfileId(p.id),
                    name: p.name,
                    correction: p.correction.try_into()?,
                })
            })
            .collect::<Result<_, ControlError>>()?,
        active_profile: document.active_profile.map(ProfileId),
    };
    settings.validate()?;
    Ok(settings)
}
