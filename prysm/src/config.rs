use std::num::{NonZeroU32, NonZeroUsize};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use prysm_core::{BrightnessPercent, CaptureResolution, Config, SampleDensity};
use serde::{Deserialize, Deserializer, de::Error};

#[derive(Debug, Deserialize)]
#[serde(from = "RawConfig")]
pub(super) struct RuntimeConfig {
    pub settings: Config,
    pub input: Input,
    pub output: Output,
}

// Mirror the flat TOML fields without coupling core to serde.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default, deserialize_with = "deserialize_brightness")]
    brightness_percent: BrightnessPercent,
    #[serde(default, deserialize_with = "deserialize_sample_density")]
    sample_density: SampleDensity,
    #[serde(default, deserialize_with = "deserialize_resolution")]
    capture_resolution: CaptureResolution,
    input: Input,
    output: Output,
}

impl RuntimeConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("Cannot read configuration {}", path.display()))?;
        toml::from_str(&contents)
            .with_context(|| format!("Invalid configuration {}", path.display()))
    }
}

impl From<RawConfig> for RuntimeConfig {
    fn from(raw: RawConfig) -> Self {
        Self {
            settings: Config {
                capture_resolution: raw.capture_resolution,
                brightness_percent: raw.brightness_percent,
                sample_density: raw.sample_density,
                ..Config::default()
            },
            input: raw.input,
            output: raw.output,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub(super) enum Input {
    Device { device: DeviceId },
    File { path: VideoPath },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub(super) enum Output {
    Wled {
        address: WledAddress,
        #[serde(default = "default_refresh_rate_hz")]
        refresh_rate_hz: NonZeroU32,
        leds: LedCounts,
    },
    Egui {
        #[serde(default = "default_refresh_rate_hz")]
        refresh_rate_hz: NonZeroU32,
    },
}

fn default_refresh_rate_hz() -> NonZeroU32 {
    NonZeroU32::new(30).unwrap()
}

/// Linux device path or macOS `AVFoundation` device UID.
#[derive(Debug, Deserialize)]
#[serde(try_from = "String")]
pub(super) struct DeviceId(String);

impl TryFrom<String> for DeviceId {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.trim().is_empty() {
            return Err("input device must not be empty");
        }
        Ok(Self(value))
    }
}

impl DeviceId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Deserialize)]
#[serde(try_from = "PathBuf")]
pub(super) struct VideoPath(PathBuf);

impl TryFrom<PathBuf> for VideoPath {
    type Error = &'static str;

    fn try_from(value: PathBuf) -> Result<Self, Self::Error> {
        if value.as_os_str().is_empty() {
            return Err("input file path must not be empty");
        }
        Ok(Self(value))
    }
}

impl VideoPath {
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

#[derive(Debug, Deserialize)]
#[serde(try_from = "String")]
pub(super) struct WledAddress(String);

impl TryFrom<String> for WledAddress {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.trim().is_empty() {
            return Err("WLED address must not be empty");
        }
        Ok(Self(value))
    }
}

impl WledAddress {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Validated WLED layout in top, right, bottom, left order.
#[derive(Debug, Deserialize)]
#[serde(try_from = "RawLedCounts")]
pub(super) struct LedCounts([usize; 4]);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLedCounts {
    top: u16,
    right: u16,
    bottom: u16,
    left: u16,
}

impl TryFrom<RawLedCounts> for LedCounts {
    type Error = &'static str;

    fn try_from(value: RawLedCounts) -> Result<Self, Self::Error> {
        let counts = [value.top, value.right, value.bottom, value.left].map(usize::from);
        let total: usize = counts.iter().sum();
        if !(1..=usize::from(u16::MAX)).contains(&total) {
            return Err("total LED count must be between 1 and 65535");
        }
        Ok(Self(counts))
    }
}

impl LedCounts {
    pub const fn counts(&self) -> [usize; 4] {
        self.0
    }
}

// Keep serde at the application boundary; core remains dependency-free.
fn deserialize_brightness<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BrightnessPercent, D::Error> {
    u8::deserialize(deserializer).map(BrightnessPercent)
}

fn deserialize_sample_density<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<SampleDensity, D::Error> {
    NonZeroUsize::deserialize(deserializer).map(|value| SampleDensity(value.get()))
}

fn deserialize_resolution<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<CaptureResolution, D::Error> {
    let [width, height]: [u32; 2] = Vec::<u32>::deserialize(deserializer)?
        .try_into()
        .map_err(|_| D::Error::custom("capture resolution must contain [width, height]"))?;
    CaptureResolution::new(width, height).map_err(D::Error::custom)
}
