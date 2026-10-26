//! Validated processing settings and conversion to the core model.
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

macro_rules! bounded {
    ($name:ident, $primitive:ty, $wire:literal, $min:expr, $max:expr) => {
        #[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
        #[serde(try_from = $wire, into = $wire)]
        pub struct $name($primitive);
        impl utoipa::PartialSchema for $name {
            fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
                use utoipa::openapi::schema::{ObjectBuilder, Type};
                ObjectBuilder::new()
                    .schema_type(if $wire == "f32" {
                        Type::Number
                    } else {
                        Type::Integer
                    })
                    .minimum(Some(f64::from($min)))
                    .maximum(Some(f64::from($max)))
                    .into()
            }
        }
        impl ToSchema for $name {}
        impl $name {
            pub fn new(value: $primitive) -> Result<Self, &'static str> {
                if ($min..=$max).contains(&value) {
                    Ok(Self(value))
                } else {
                    Err(concat!(stringify!($name), " is outside its allowed range"))
                }
            }
            #[must_use]
            pub const fn get(self) -> $primitive {
                self.0
            }
        }
        impl TryFrom<$primitive> for $name {
            type Error = &'static str;
            fn try_from(value: $primitive) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }
        impl From<$name> for $primitive {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}
bounded!(BrightnessPercent, u8, "u8", 0, 100);
bounded!(ShadowLiftPercent, u8, "u8", 0, 100);
bounded!(SpatialReachPercent, u8, "u8", 1, 100);
bounded!(SampleDensity, u32, "u32", 1, u32::MAX);
bounded!(SmoothingSeconds, f32, "f32", 0.0, f32::MAX);

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessingSettings {
    pub brightness_percent: BrightnessPercent,
    pub shadow_lift_percent: ShadowLiftPercent,
    pub smoothing_seconds: SmoothingSeconds,
    pub sample_density: SampleDensity,
    pub spatial_reach_percent: SpatialReachPercent,
    pub remove_black_bars: bool,
}
impl Default for ProcessingSettings {
    fn default() -> Self {
        let defaults = prysm_core::Config::default();
        Self {
            brightness_percent: BrightnessPercent(defaults.brightness_percent.0),
            shadow_lift_percent: ShadowLiftPercent(defaults.shadow_lift_percent.0),
            smoothing_seconds: SmoothingSeconds(defaults.smoothing_seconds.0),
            sample_density: SampleDensity(
                u32::try_from(defaults.sample_density.get())
                    .expect("default sample density fits u32"),
            ),
            spatial_reach_percent: SpatialReachPercent(defaults.spatial_reach_percent.0),
            remove_black_bars: defaults.remove_black_bars,
        }
    }
}
impl ProcessingSettings {
    #[must_use]
    pub fn to_core(self) -> prysm_core::Config {
        prysm_core::Config {
            brightness_percent: prysm_core::BrightnessPercent(self.brightness_percent.get()),
            shadow_lift_percent: prysm_core::ShadowLiftPercent(self.shadow_lift_percent.get()),
            smoothing_seconds: prysm_core::SmoothingSeconds(self.smoothing_seconds.get()),
            sample_density: prysm_core::SampleDensity(self.sample_density.get() as usize),
            spatial_reach_percent: prysm_core::SpatialReachPercent(
                self.spatial_reach_percent.get(),
            ),
            remove_black_bars: self.remove_black_bars,
        }
    }
}
