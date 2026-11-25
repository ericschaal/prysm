//! What the TV app sees: the published state and its parts.
use crate::preferences::Preferences;
use prysm_webos_link::{Fix, FullStatus, LightingTarget, OutputIssue, SlimLink, SlimStatus, Video};
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

impl Flavor {
    /// The serialized name, such as `full`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Slim => "slim",
        }
    }
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
    /// Why the saved preferences could not be read: the service runs with
    /// the defaults and keeps the file as it is until the app saves
    /// preferences. Nothing stopped for it.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schema(nullable = false))]
    pub unreadable: Option<String>,
    /// The latest log lines of the supervisor and the worker, oldest first
    /// (at most 20).
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
        target: LightingTarget,
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

    /// What the person is asked, in whichever phase says it.
    pub(crate) fn problem(&self) -> Option<&Problem> {
        match self {
            Self::Full {
                worker: FullWorker::Running { problem, .. } | FullWorker::Stopping { problem, .. },
                ..
            }
            | Self::Slim {
                worker: SlimWorker::Stopping { problem },
                ..
            } => problem.as_ref(),
            Self::Full {
                worker: FullWorker::Asking { problem },
                ..
            }
            | Self::Slim {
                worker: SlimWorker::Asking { problem },
                ..
            } => Some(problem),
            Self::Full {
                worker: FullWorker::Stopped,
                ..
            }
            | Self::Slim {
                worker: SlimWorker::Stopped | SlimWorker::Running { .. },
                ..
            } => None,
        }
    }
}

/// What the person is asked: what nothing on the TV recovers, as
/// `{ "step": … }`, with the person's next step for `ask`. Open: an app
/// reads a step it does not know as `ask` with `try_again`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "step", content = "fix", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum Asked {
    /// The lights do not answer, and Prysm stopped trying.
    Unanswered,
    /// The person must act, by this step.
    #[cfg_attr(feature = "schema", schema(value_type = crate::schema::FixSchema))]
    Ask(Fix),
}

/// A failure the person is asked about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Problem {
    pub asked: Asked,
    /// What failed, with its causes.
    pub message: String,
}

/// The full package's worker process, as `{ "phase": … }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum FullWorker {
    Stopped,
    Running {
        /// Screen sync runs and frames arrive (false while it waits for
        /// video).
        capturing: bool,
        /// Frames per second over about the last second, rounded, while
        /// screen sync runs; 0 while it waits for frames.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schema", schema(nullable = false))]
        fps: Option<u16>,
        /// Why the lights do not show what was chosen, as the worker last
        /// reported it: with screen sync any issue, `unreachable` also while
        /// it waits for lights that do not answer; with off or effects
        /// `unreachable` alone, while the lights do not answer the request
        /// that puts them in that mode (tried again until they do).
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(
            feature = "schema",
            schema(value_type = Option<crate::schema::OutputIssueSchema>, nullable = false)
        )]
        lights: Option<OutputIssue>,
        /// The portal's LAN address once the worker reported it.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schema", schema(nullable = false))]
        portal: Option<String>,
        /// What the worker asks of the person: it keeps running, since its
        /// portal is where settings change.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schema", schema(nullable = false))]
        problem: Option<Problem>,
    },
    /// Being stopped; the portal may still answer until it exits.
    Stopping {
        /// The portal's LAN address the worker last reported.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schema", schema(nullable = false))]
        portal: Option<String>,
        /// What the person is asked once it exited; absent when a new
        /// worker follows (a restart), or none is wanted.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schema", schema(nullable = false))]
        problem: Option<Problem>,
    },
    /// No worker, and no portal: the person is asked. Choosing a target
    /// (`setTarget`) answers it and starts the worker again.
    Asking {
        problem: Problem,
    },
}

/// The slim package's worker process, as `{ "phase": … }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum SlimWorker {
    Stopped,
    Running {
        /// Frames were sent in the last second (so also just after the
        /// stream ended).
        capturing: bool,
        /// Where the uplink to the receiver stands, as the worker last
        /// reported it; it says the worker's own waits and failures.
        #[cfg_attr(feature = "schema", schema(value_type = crate::schema::SlimLinkSchema))]
        link: SlimLink,
    },
    Stopping {
        /// What the person is asked once it exited; absent when a new
        /// worker follows (a restart), or none is wanted.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schema", schema(nullable = false))]
        problem: Option<Problem>,
    },
    /// No worker: the person is asked. Starting capture (`setEnabled`
    /// true, also after stopping it) answers it and starts the worker
    /// again.
    Asking {
        problem: Problem,
    },
}

/// Where the worker process is, the same for both packages, with what the
/// person is asked there; each package's worker type adds what its reports
/// say in that phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Process {
    Stopped,
    /// What the running worker asks; only a full worker's is published (a
    /// slim worker's link says its own).
    Running(Option<Problem>),
    /// What the person is asked once it exited.
    Stopping(Option<Problem>),
    Asking(Problem),
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
            Process::Running(problem) => Self::Running {
                capturing: report.is_some_and(|report| report.capturing),
                fps: report.and_then(|report| report.fps),
                lights: report.and_then(|report| report.lights),
                portal: portal(),
                problem,
            },
            Process::Stopping(problem) => Self::Stopping {
                portal: portal(),
                problem,
            },
            Process::Asking(problem) => Self::Asking { problem },
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
            Process::Running(_) => {
                let SlimStatus { capturing, link } = report.cloned().unwrap_or_default();
                Self::Running { capturing, link }
            }
            Process::Stopping(problem) => Self::Stopping { problem },
            Process::Asking(problem) => Self::Asking { problem },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only a running worker says the lights' issue its last report gave.
    #[test]
    fn a_running_full_worker_says_the_lights_issue() {
        let report = FullStatus {
            capturing: true,
            fps: Some(30),
            target: LightingTarget::ScreenSync,
            lights: Some(OutputIssue::Unreachable),
            applied: 0,
            portal: None,
            failure: None,
        };
        assert_eq!(
            FullWorker::new(Process::Running(None), Some(&report)),
            FullWorker::Running {
                capturing: true,
                fps: Some(30),
                lights: Some(OutputIssue::Unreachable),
                portal: None,
                problem: None,
            }
        );
        let json =
            serde_json::to_value(FullWorker::new(Process::Running(None), Some(&report))).unwrap();
        assert_eq!(json["lights"], "unreachable");
        for process in [Process::Stopped, Process::Stopping(None)] {
            let json =
                serde_json::to_value(FullWorker::new(process.clone(), Some(&report))).unwrap();
            assert_eq!(json.get("lights"), None, "{process:?}");
        }
        let fine = FullStatus {
            lights: None,
            ..report
        };
        let json =
            serde_json::to_value(FullWorker::new(Process::Running(None), Some(&fine))).unwrap();
        assert_eq!(json.get("lights"), None, "absent, never null");
    }

    /// What is asked is tagged by `step`, with the fix only for `ask`, as
    /// the worker link's escalation is.
    #[test]
    fn what_is_asked_has_these_json_shapes() {
        assert_eq!(
            serde_json::to_value(Asked::Unanswered).unwrap(),
            serde_json::json!({ "step": "unanswered" })
        );
        for fix in Fix::ALL {
            let json = serde_json::to_value(Asked::Ask(fix)).unwrap();
            assert_eq!(
                json,
                serde_json::json!({ "step": "ask", "fix": serde_json::to_value(fix).unwrap() })
            );
            assert_eq!(
                json,
                serde_json::to_value(prysm_webos_link::Escalation::Ask(fix)).unwrap()
            );
        }
    }

    /// `flavor` in the state is the [`Flavor`] the rest of the service uses.
    #[test]
    fn the_package_tag_is_the_flavor() {
        for package in [
            Package::Full {
                target: LightingTarget::Off,
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
            assert_eq!(json["flavor"], package.flavor().as_str());
        }
    }
}
