//! TOML parsing and validation at the application boundary.

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

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_CONFIG: &str = r#"
[input]
type = "file"
path = "/tmp/my video.mp4"
[output]
type = "wled"
address = "wled.local:4048"
[output.leds]
top = 96
right = 54
bottom = 96
left = 54
"#;

    fn parse(contents: &str) -> Result<RuntimeConfig> {
        let config: RuntimeConfig = toml::from_str(contents)?;
        Ok(config)
    }

    #[test]
    fn parses_all_input_output_combinations_and_shipped_configs() {
        let wled_output = &TEST_CONFIG[TEST_CONFIG.find("[output]").unwrap()..];
        for input in [
            "[input]\ntype = 'device'\ndevice = '/dev/video2'\n",
            "[input]\ntype = 'device'\ndevice = '0x2200000345f2131'\n",
            "[input]\ntype = 'file'\npath = '/tmp/my video.mp4'\n",
        ] {
            for output in ["[output]\ntype = 'egui'\n", wled_output] {
                let config = parse(&format!("{input}{output}")).unwrap();
                assert_eq!(config.settings, Config::default());
            }
        }
        let config = parse(TEST_CONFIG).unwrap();
        assert!(
            matches!(config.input, Input::File { path } if path.as_path() == Path::new("/tmp/my video.mp4"))
        );
        assert!(
            matches!(config.output, Output::Wled { address, refresh_rate_hz, leds } if address.as_str() == "wled.local:4048" && refresh_rate_hz.get() == 30 && leds.counts() == [96, 54, 96, 54])
        );
        parse(include_str!("../../prysm.toml")).unwrap();
        parse(include_str!("../debian/prysm.toml")).unwrap();
    }

    #[test]
    fn parses_custom_wled_refresh_rate_and_rejects_invalid_rates() {
        let config =
            parse(&TEST_CONFIG.replace("[output.leds]", "refresh_rate_hz = 60\n[output.leds]"))
                .unwrap();
        assert!(
            matches!(config.output, Output::Wled { refresh_rate_hz, .. } if refresh_rate_hz.get() == 60)
        );
        for value in ["0", "-1", "1.5", "'fast'", "4294967296"] {
            let contents = TEST_CONFIG.replace(
                "[output.leds]",
                &format!("refresh_rate_hz = {value}\n[output.leds]"),
            );
            assert!(
                parse(&contents).is_err(),
                "accepted refresh_rate_hz = {value}"
            );
        }
    }

    #[test]
    fn parses_egui_refresh_rate_and_rejects_invalid_rates() {
        let contents =
            "[input]\ntype = 'device'\ndevice = '/dev/video0'\n[output]\ntype = 'egui'\n";
        let config = parse(contents).unwrap();
        assert!(
            matches!(config.output, Output::Egui { refresh_rate_hz } if refresh_rate_hz.get() == 30)
        );
        let config = parse(&format!("{contents}refresh_rate_hz = 60")).unwrap();
        assert!(
            matches!(config.output, Output::Egui { refresh_rate_hz } if refresh_rate_hz.get() == 60)
        );
        for value in ["0", "-1", "1.5", "'fast'", "4294967296"] {
            assert!(
                parse(&format!("{contents}refresh_rate_hz = {value}")).is_err(),
                "accepted refresh_rate_hz = {value}"
            );
        }
    }

    #[test]
    fn applies_custom_processor_settings_and_capture_resolution() {
        let config = parse(&format!("brightness_percent = 60\nsample_density = 200\ncapture_resolution = [1280, 720]\n{TEST_CONFIG}")).unwrap();
        assert_eq!(config.settings.capture_resolution.dimensions(), [1280, 720]);
        let settings = config.settings;
        assert_eq!(settings.brightness_percent, BrightnessPercent(60));
        assert_eq!(settings.sample_density, SampleDensity(200));
    }

    #[test]
    fn rejects_invalid_configurations_and_counts() {
        let contents = TEST_CONFIG;
        for invalid in [
            String::new(),
            format!("sample_density = 0\n{contents}"),
            format!("sample_density = -1\n{contents}"),
            format!("capture_resolution = [0, 360]\n{contents}"),
            format!("capture_resolution = [640, 0]\n{contents}"),
            format!("capture_resolution = [4294967295, 4294967295]\n{contents}"),
            format!("capture_resolution = [-1, 360]\n{contents}"),
            format!("capture_resolution = [640]\n{contents}"),
            format!("capture_resolution = [640, 360, 1]\n{contents}"),
            contents.replace("top = 96", "top = -1"),
            contents.replace("top = 96", "top = 65536"),
            contents.replace("top = 96", "top = 65535"),
            contents.replace("top = 96", "top = 'red'"),
            contents.replace("top = 96", "tpo = 96"),
            contents
                .replace("top = 96", "top = 0")
                .replace("right = 54", "right = 0")
                .replace("bottom = 96", "bottom = 0")
                .replace("left = 54", "left = 0"),
            contents.replace("type = \"file\"", "type = \"unknown\""),
            contents.replace("type = \"file\"", "type = \"camera\""),
            contents.replace("type = \"file\"", "type = \"device\""),
            contents.replace("path = \"/tmp/my video.mp4\"", ""),
            contents.replace("path = \"/tmp/my video.mp4\"", "path = ''"),
            contents.replace("path = \"/tmp/my video.mp4\"", "device = '/dev/video0'"),
            contents.replace("type = \"wled\"", "type = \"egui\""),
            contents.replace("type = \"wled\"", "type = \"unknown\""),
            contents.replace("address =", "unknown ="),
            contents.replace("address = \"wled.local:4048\"", "address = ''"),
            format!("brightness_percent = -1\n{contents}"),
            format!("brightness_percent = 256\n{contents}"),
            format!("brightness_percent = 'bright'\n{contents}"),
            "[input]\ntype = 'device'\n[output]\ntype = 'egui'".to_owned(),
            "[input]\ntype = 'device'\ndevice = ''\n[output]\ntype = 'egui'".to_owned(),
        ] {
            assert!(parse(&invalid).is_err(), "accepted {invalid:?}");
        }
    }
}
