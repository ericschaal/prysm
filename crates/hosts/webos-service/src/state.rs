//! What the TV app sees: the published state and its parts.
use crate::preferences::Preferences;
use prysm_webos_link::{
    FullStatus, LightingTarget, SlimEncoding, SlimStatus, Video, WorkerFailure,
};
use serde::Serialize;

/// Which package this supervisor belongs to. Published as [`Package`]'s tag.
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

/// The `getState` reply and subscription payload (plus LS2's `returnValue`):
/// the fields both packages have, and the installed package's own
/// ([`Package`], tagged by `flavor`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct State {
    #[serde(flatten)]
    pub package: Package,
    pub preferences: Preferences,
    pub power: Power,
    #[cfg_attr(feature = "schema", schema(value_type = crate::schema::VideoSchema))]
    pub video: Video,
    /// The current failure (or, slim, the receiver wait): what it is and
    /// its message with the causes.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "schema",
        schema(value_type = Option<crate::schema::WorkerFailureSchema>, nullable = false)
    )]
    pub failure: Option<WorkerFailure>,
    /// The latest worker stderr lines, oldest first (at most 20).
    pub log: Vec<String>,
}

/// The installed package (both install as the same app), what the user asked
/// of it and its worker process, as `{ "flavor": … }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "flavor", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum Package {
    /// `prysm-webos`: capture, processing, WLED and the portal on the TV.
    Full {
        /// The engine's target, including changes made in the portal.
        #[cfg_attr(
            feature = "schema",
            schema(value_type = crate::schema::LightingTargetSchema)
        )]
        lighting: LightingTarget,
        worker: FullWorker,
    },
    /// `prysm-webos-slim`: capture forwarded to a headless Prysm.
    Slim {
        /// Frames should flow to the receiver.
        enabled: bool,
        worker: SlimWorker,
    },
}

#[cfg(test)]
impl Package {
    pub(crate) fn flavor(&self) -> Flavor {
        match self {
            Self::Full { .. } => Flavor::Full,
            Self::Slim { .. } => Flavor::Slim,
        }
    }
}

/// The full package's worker process, as `{ "phase": … }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum FullWorker {
    Stopped,
    Running {
        /// Lighting runs and frames arrive (false while it waits for video).
        capturing: bool,
        /// Frames per second over about the last second, rounded, while
        /// lighting runs; 0 while it waits for frames.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schema", schema(nullable = false))]
        fps: Option<u16>,
        /// The portal's LAN address once the worker reported it.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schema", schema(nullable = false))]
        portal: Option<String>,
    },
    /// Being stopped; the portal may still answer until it exits.
    Stopping {
        /// The portal's LAN address the worker last reported.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schema", schema(nullable = false))]
        portal: Option<String>,
    },
    /// Waiting before the next start after a failure.
    Retrying {
        /// Whole seconds until the next start, rounded up, when this state
        /// was sent (at least 1). The app counts down from when it received
        /// it; no clock is shared.
        seconds: u32,
    },
}

/// The slim package's worker process, as `{ "phase": … }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum SlimWorker {
    Stopped,
    Running {
        /// Frames were sent in the last second.
        capturing: bool,
        /// Frames sent per second over about the last second, rounded, while
        /// connected and sending; 0 while it waits for frames.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schema", schema(nullable = false))]
        fps: Option<u16>,
        /// The encoding of the frames being sent while connected to the
        /// receiver; always the one the receiver requested (set in the
        /// desktop portal's remote capture settings).
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(
            feature = "schema",
            schema(value_type = Option<crate::schema::SlimEncodingSchema>, nullable = false)
        )]
        encoding: Option<SlimEncoding>,
        /// The receiver stopped the stream because its lighting is off; the
        /// TV capture is released until a receiver takes a connection
        /// again. Not a failure.
        paused: bool,
    },
    Stopping,
    /// Waiting before the next start after a failure.
    Retrying {
        /// Whole seconds until the next start, rounded up, when this state
        /// was sent (at least 1). The app counts down from when it received
        /// it; no clock is shared.
        seconds: u32,
    },
}

/// Where the worker process is, the same for both packages; each package's
/// worker type adds what its reports say in that phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Process {
    Stopped,
    Running,
    Stopping,
    Retrying { seconds: u32 },
}

impl FullWorker {
    /// `process` with the running worker's last `report` (none before its
    /// first). Only a running worker's capture counts: one being stopped is
    /// not capturing for the app.
    #[must_use]
    pub(crate) fn new(process: Process, report: Option<&FullStatus>) -> Self {
        let portal = || report.and_then(|report| report.portal.clone());
        match process {
            Process::Stopped => Self::Stopped,
            Process::Running => Self::Running {
                capturing: report.is_some_and(|report| report.capturing),
                fps: report.and_then(|report| report.fps),
                portal: portal(),
            },
            Process::Stopping => Self::Stopping { portal: portal() },
            Process::Retrying { seconds } => Self::Retrying { seconds },
        }
    }
}

impl SlimWorker {
    /// `process` with the running worker's last `report` (none before its
    /// first). Only a running worker's stream counts.
    #[must_use]
    pub(crate) fn new(process: Process, report: Option<&SlimStatus>) -> Self {
        match process {
            Process::Stopped => Self::Stopped,
            Process::Running => {
                let report = report.cloned().unwrap_or_default();
                Self::Running {
                    capturing: report.capturing,
                    fps: report.fps,
                    encoding: report.encoding,
                    paused: report.paused,
                }
            }
            Process::Stopping => Self::Stopping,
            Process::Retrying { seconds } => Self::Retrying { seconds },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `flavor` in the state is the [`Flavor`] the rest of the service uses.
    #[test]
    fn the_package_tag_is_the_flavor() {
        for package in [
            Package::Full {
                lighting: LightingTarget::Off,
                worker: FullWorker::Stopped,
            },
            Package::Slim {
                enabled: false,
                worker: SlimWorker::Stopped,
            },
        ] {
            let json = serde_json::to_value(&package).unwrap();
            assert_eq!(
                json["flavor"],
                serde_json::to_value(package.flavor()).unwrap()
            );
        }
    }
}
