//! Complete settings representation shared by file and command adapters.
//! Domain settings remain independent of serialization.
use prysm_contract::{
    ControlError,
    application::Settings as DomainSettings,
    calibration::{CalibrationProfile, ProfileId},
};
use prysm_core::{self as core, ColorCorrection, Config};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsDocument<C, O> {
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

impl<C, O> TryFrom<DomainSettings<C, O>> for SettingsDocument<C, O> {
    type Error = ControlError;

    fn try_from(settings: DomainSettings<C, O>) -> Result<Self, Self::Error> {
        settings.validate()?;
        Ok(Self {
            capture: settings.capture,
            output: settings.output,
            processing: settings.processing.into(),
            profiles: settings
                .profiles
                .into_iter()
                .map(|profile| Profile {
                    id: profile.id.0,
                    name: profile.name,
                    correction: profile.correction.into(),
                })
                .collect(),
            active_profile: settings.active_profile.map(|id| id.0),
        })
    }
}

impl<C, O> TryFrom<SettingsDocument<C, O>> for DomainSettings<C, O> {
    type Error = ControlError;

    fn try_from(document: SettingsDocument<C, O>) -> Result<Self, Self::Error> {
        let settings = Self {
            capture: document.capture,
            output: document.output,
            processing: document.processing.try_into()?,
            profiles: document
                .profiles
                .into_iter()
                .map(|profile| {
                    Ok(CalibrationProfile {
                        id: ProfileId(profile.id),
                        name: profile.name,
                        correction: profile.correction.try_into()?,
                    })
                })
                .collect::<Result<_, ControlError>>()?,
            active_profile: document.active_profile.map(ProfileId),
        };
        settings.validate()?;
        Ok(settings)
    }
}
