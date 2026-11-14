//! Worker argv. The supervisor builds it with `to_args`; the worker parses it
//! with clap ([`crate::worker::args`]); a rejection is a packaging bug (exit `USAGE`).
use crate::PrysmAddress;
use clap::Parser;
use prysm_ports::LightingTarget;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;

/// `prysm-webos --settings PATH --listen ADDR --lighting off|processed|device_native`
#[derive(Debug, Clone, PartialEq, Eq, Parser)]
pub struct FullArgs {
    /// The engine's settings file
    #[arg(long, value_name = "PATH")]
    pub settings: PathBuf,
    /// Portal listen address, 0.0.0.0:8787 on the TV
    #[arg(long, value_name = "ADDR")]
    pub listen: SocketAddr,
    /// Initial lighting target: off, processed or device_native
    #[arg(long, value_name = "TARGET", value_parser = lighting_target)]
    pub lighting: LightingTarget,
}

/// `prysm-webos-slim --prysm URL`. What to capture and how to encode it
/// comes from Prysm (the remote source's stream request), not from the TV.
#[derive(Debug, Clone, PartialEq, Eq, Parser)]
pub struct SlimArgs {
    /// The Prysm to stream to: its portal address, http:// or https://
    #[arg(long, value_name = "URL")]
    pub prysm: PrysmAddress,
}

/// The encoding of the frames a slim worker sends: the one its receiver
/// requested. Exactly that encoder is used: one that cannot start or fails
/// is reported, never a switch to another.
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

impl FullArgs {
    #[must_use]
    pub fn to_args(&self) -> Vec<String> {
        vec![
            "--settings".into(),
            self.settings.display().to_string(),
            "--listen".into(),
            self.listen.to_string(),
            "--lighting".into(),
            self.lighting.as_str().into(),
        ]
    }
}

impl SlimArgs {
    #[must_use]
    pub fn to_args(&self) -> Vec<String> {
        vec!["--prysm".into(), self.prysm.to_string()]
    }
}

/// `--lighting` values are the serde names of [`LightingTarget`].
fn lighting_target(name: &str) -> Result<LightingTarget, String> {
    LightingTarget::ALL
        .into_iter()
        .find(|target| target.as_str() == name)
        .ok_or_else(|| format!("unknown target `{name}`"))
}
