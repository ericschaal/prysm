//! Worker argv. The supervisor builds it with `to_args`; the worker parses it
//! with `parse`; a rejection is a packaging bug (exit `USAGE`).
use prysm_ports::LightingTarget;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;

/// `prysm-webos --settings PATH --listen ADDR --lighting off|processed|device_native`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullArgs {
    /// The engine's settings file.
    pub settings: PathBuf,
    /// Portal listen address, `0.0.0.0:8787` on the TV.
    pub listen: SocketAddr,
    /// Initial lighting target.
    pub lighting: LightingTarget,
}

/// `prysm-webos-slim --receiver ADDR --width N --height N --fps N --include-ui BOOL
/// --encoding h264_hardware|h264_software|raw`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlimArgs {
    /// The headless host's frame receiver.
    pub receiver: SocketAddr,
    pub capture: SlimCapture,
}

/// Capture size, rate and menu blending a slim worker sends, and how the
/// frames are encoded. Same capture rules as the `webos` capture settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SlimCapture {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Blend TV menus into the capture.
    pub include_ui: bool,
    pub encoding: SlimEncoding,
}

impl Default for SlimCapture {
    fn default() -> Self {
        Self {
            width: 320,
            height: 180,
            fps: 30,
            include_ui: true,
            encoding: SlimEncoding::default(),
        }
    }
}

/// How the slim worker encodes the frames it sends. Exactly this encoder is
/// used: one that cannot start or fails is a worker failure, never a switch
/// to another.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlimEncoding {
    /// The TV's hardware H.264 encoder (about 2 Mbit/s).
    #[default]
    H264Hardware,
    /// OpenH264 on the TV's CPU (about 2 Mbit/s).
    H264Software,
    /// Uncompressed NV12 (320x180 at 30 fps is about 21 Mbit/s).
    Raw,
}

impl SlimEncoding {
    const ALL: [Self; 3] = [Self::H264Hardware, Self::H264Software, Self::Raw];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::H264Hardware => "h264_hardware",
            Self::H264Software => "h264_software",
            Self::Raw => "raw",
        }
    }
}

impl std::fmt::Display for SlimEncoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for SlimEncoding {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|encoding| encoding.name() == name)
            .ok_or_else(|| "expected h264_hardware, h264_software or raw".into())
    }
}

impl SlimCapture {
    /// # Errors
    /// The first invalid field, as `capture.<field>: <reason>`.
    pub fn validate(&self) -> Result<(), String> {
        let even = |field: &str, value: u32, max: u32| {
            if (2..=max).contains(&value) && value.is_multiple_of(2) {
                Ok(())
            } else {
                Err(format!(
                    "capture.{field}: must be an even number from 2 to {max}"
                ))
            }
        };
        even("width", self.width, 1920)?;
        even("height", self.height, 1080)?;
        if !(1..=60).contains(&self.fps) {
            return Err("capture.fps: must be 1 to 60".into());
        }
        Ok(())
    }
}

impl FullArgs {
    #[must_use]
    pub fn to_args(&self) -> Vec<String> {
        vec![
            "--settings".into(),
            self.settings.display().to_string(),
            "--listen".into(),
            self.listen.to_string(),
            "--lighting".into(),
            target_name(self.lighting),
        ]
    }

    /// # Errors
    /// A missing, repeated, unknown or invalid option.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = Options::parse(args, &["settings", "listen", "lighting"])?;
        Ok(Self {
            settings: PathBuf::from(options.take("settings")?),
            listen: options.value("listen")?,
            lighting: target(&options.take("lighting")?)?,
        })
    }
}

impl SlimArgs {
    #[must_use]
    pub fn to_args(&self) -> Vec<String> {
        let c = &self.capture;
        [
            ("--receiver", self.receiver.to_string()),
            ("--width", c.width.to_string()),
            ("--height", c.height.to_string()),
            ("--fps", c.fps.to_string()),
            ("--include-ui", c.include_ui.to_string()),
            ("--encoding", c.encoding.to_string()),
        ]
        .into_iter()
        .flat_map(|(key, value)| [key.to_owned(), value])
        .collect()
    }

    /// # Errors
    /// A missing, repeated, unknown or invalid option.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = Options::parse(
            args,
            &[
                "receiver",
                "width",
                "height",
                "fps",
                "include-ui",
                "encoding",
            ],
        )?;
        let parsed = Self {
            receiver: options.value("receiver")?,
            capture: SlimCapture {
                width: options.value("width")?,
                height: options.value("height")?,
                fps: options.value("fps")?,
                include_ui: options.value("include-ui")?,
                encoding: options.value("encoding")?,
            },
        };
        parsed.capture.validate()?;
        Ok(parsed)
    }
}

/// `--lighting` values are the serde names of [`LightingTarget`].
fn target_name(target: LightingTarget) -> String {
    match serde_json::to_value(target) {
        Ok(serde_json::Value::String(name)) => name,
        other => unreachable!("LightingTarget serializes to a string, not {other:?}"),
    }
}

fn target(name: &str) -> Result<LightingTarget, String> {
    serde_json::from_value(serde_json::Value::String(name.into()))
        .map_err(|_| format!("--lighting: unknown target `{name}`"))
}

/// `--key value` pairs; every key in `keys` exactly once.
struct Options(BTreeMap<String, String>);

impl Options {
    fn parse(args: &[String], keys: &[&str]) -> Result<Self, String> {
        let mut values = BTreeMap::new();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let key = arg
                .strip_prefix("--")
                .filter(|key| keys.contains(key))
                .ok_or_else(|| format!("unexpected argument `{arg}`"))?;
            let value = args.next().ok_or_else(|| format!("{arg} needs a value"))?;
            if values.insert(key.to_owned(), value.clone()).is_some() {
                return Err(format!("{arg} given twice"));
            }
        }
        Ok(Self(values))
    }

    fn take(&mut self, key: &str) -> Result<String, String> {
        self.0
            .remove(key)
            .ok_or_else(|| format!("--{key} is required"))
    }

    fn value<T: FromStr>(&mut self, key: &str) -> Result<T, String>
    where
        T::Err: std::fmt::Display,
    {
        let value = self.take(key)?;
        value
            .parse()
            .map_err(|error| format!("--{key} `{value}`: {error}"))
    }
}
