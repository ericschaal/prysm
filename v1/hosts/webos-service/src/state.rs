//! What the TV app sees: the published state and its parts.
use crate::preferences::Preferences;
use prysm_webos_link::{FailureKind, LightingTarget, SlimEncoding, Video};
use serde::Serialize;

/// Which package this supervisor belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum Flavor {
    /// `prysm-webos`: capture, processing, WLED and the portal on the TV.
    Full,
    /// `prysm-webos-slim`: capture forwarded to a headless Prysm.
    Slim,
}

/// TV power as `com.webos.service.tvpower` reports it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum Phase {
    Stopped,
    Running,
    Stopping,
    /// Waiting before the next start after a failure.
    Retrying,
}

/// The `getState` reply and subscription payload (plus LS2's `returnValue`).
/// Fields marked "Full only" are `null` in the slim package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct State {
    /// The installed package; both install as `io.prysm.tv`.
    pub flavor: Flavor,
    pub preferences: Preferences,
    /// Slim: frames should flow. Full: lighting is not off.
    pub enabled: bool,
    /// Full only: the engine's target, including changes made in the portal.
    #[cfg_attr(
        feature = "schema",
        schema(value_type = Option<crate::schema::LightingTargetSchema>)
    )]
    pub lighting: Option<LightingTarget>,
    pub power: Power,
    #[cfg_attr(feature = "schema", schema(value_type = crate::schema::VideoSchema))]
    pub video: Video,
    pub worker: Phase,
    /// Frames are flowing. Full: lighting runs and frames arrive (false
    /// while it waits for video). Slim: frames were sent in the last second.
    /// Always false unless `worker` is `running`.
    pub capturing: bool,
    /// Full only: the portal's LAN address once the worker reported it.
    pub portal: Option<String>,
    /// The current failure (or, slim, the receiver wait) with its causes.
    pub failure: Option<String>,
    /// What `failure` is; set exactly when `failure` is.
    #[cfg_attr(
        feature = "schema",
        schema(value_type = Option<crate::schema::FailureKindSchema>)
    )]
    pub failure_kind: Option<FailureKind>,
    /// Slim only: the encoding of the frames being sent while connected to
    /// the receiver; always the one the receiver requested (set in the
    /// desktop portal's remote capture settings).
    #[cfg_attr(
        feature = "schema",
        schema(value_type = Option<crate::schema::SlimEncodingSchema>)
    )]
    pub encoding: Option<SlimEncoding>,
    /// The latest worker stderr lines, oldest first (at most 20).
    pub log: Vec<String>,
}
