//! Worker argv. The supervisor builds it with `to_args`; the worker parses it
//! with clap ([`crate::worker::args`]); a rejection is a packaging bug (exit `USAGE`).
use crate::PrysmAddress;
use clap::Parser;
use prysm_ports::LightingTarget;
use std::net::SocketAddr;
use std::path::PathBuf;

/// `prysm-webos --settings PATH --listen ADDR --target off|screen_sync|effects`
#[derive(Debug, Clone, PartialEq, Eq, Parser)]
pub struct FullArgs {
    /// The engine's settings file
    #[arg(long, value_name = "PATH")]
    pub settings: PathBuf,
    /// Portal listen address, 0.0.0.0:8787 on the TV
    #[arg(long, value_name = "ADDR")]
    pub listen: SocketAddr,
    /// Initial lighting target: off, screen_sync or effects
    #[arg(long, value_name = "TARGET", value_parser = lighting_target)]
    pub target: LightingTarget,
}

/// `prysm-webos-slim --prysm URL`. What to capture and how to encode it
/// comes from Prysm (the remote source's stream request), not from the TV.
#[derive(Debug, Clone, PartialEq, Eq, Parser)]
pub struct SlimArgs {
    /// The Prysm to stream to: its portal address, http:// or https://
    #[arg(long, value_name = "URL")]
    pub prysm: PrysmAddress,
}

impl FullArgs {
    #[must_use]
    pub fn to_args(&self) -> Vec<String> {
        vec![
            "--settings".into(),
            self.settings.display().to_string(),
            "--listen".into(),
            self.listen.to_string(),
            "--target".into(),
            self.target.as_str().into(),
        ]
    }
}

impl SlimArgs {
    #[must_use]
    pub fn to_args(&self) -> Vec<String> {
        vec!["--prysm".into(), self.prysm.to_string()]
    }
}

/// `--target` values are the serde names of [`LightingTarget`].
fn lighting_target(name: &str) -> Result<LightingTarget, String> {
    LightingTarget::ALL
        .into_iter()
        .find(|target| target.as_str() == name)
        .ok_or_else(|| format!("unknown target `{name}`"))
}
