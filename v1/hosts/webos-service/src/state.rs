//! What the TV app sees: the published state and its parts.
use crate::preferences::Preferences;
use prysm_webos_link::{LightingTarget, Video};
use serde::Serialize;

/// Which package this supervisor belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Flavor {
    /// `prysm-webos`: capture, processing, WLED and the portal on the TV.
    Full,
    /// `prysm-webos-slim`: capture forwarded to a headless Prysm.
    Slim,
}

/// TV power as `com.webos.service.tvpower` reports it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Power {
    /// No valid report (yet). Fails closed: not awake.
    #[default]
    Unknown,
    Active,
    ScreenSaver,
    /// Any other state: standby, suspend, power off.
    Asleep,
}

impl Power {
    /// The worker runs while awake; capture only while Active.
    #[must_use]
    pub fn awake(self) -> bool {
        matches!(self, Self::Active | Self::ScreenSaver)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Stopped,
    Running,
    Stopping,
    /// Waiting before the next start after a failure.
    Retrying,
}

/// The `getState` reply and subscription payload (plus LS2's `returnValue`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct State {
    pub flavor: Flavor,
    pub preferences: Preferences,
    /// Slim: frames should flow. Full: lighting is not off.
    pub enabled: bool,
    /// Full only: the engine's target, including changes made in the portal.
    pub lighting: Option<LightingTarget>,
    pub power: Power,
    pub video: Video,
    pub worker: Phase,
    pub capturing: bool,
    /// Full only: the portal's LAN address once the worker reported it.
    pub portal: Option<String>,
    pub failure: Option<String>,
    /// The latest worker stderr lines, oldest first.
    pub log: Vec<String>,
}
