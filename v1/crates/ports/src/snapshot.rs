use crate::{Error, Revision, Settings};
use serde::{Deserialize, Serialize};
use std::fmt;

/// What the user wants the LEDs to do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightingTarget {
    #[default]
    Off,
    /// Colors processed from captured frames.
    Processed,
    /// The output's own effect; no capture.
    DeviceNative,
}

/// What the runtime is actually doing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SessionState {
    /// No session; the output (if any was touched) is idle.
    #[default]
    Stopped,
    Running,
    /// The output was put in its native idle mode.
    Native,
    /// A session did not stop in time. Only shutdown is accepted.
    Faulted(SessionPhase),
}

/// Where a session thread is; reported when it fails to stop in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPhase {
    Running,
    ClosingSource,
    IdlingOutput,
    Finished,
}

impl fmt::Display for SessionPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Running => "running",
            Self::ClosingSource => "closing the source",
            Self::IdlingOutput => "idling the output",
            Self::Finished => "finishing",
        })
    }
}

/// The runtime's observable state. Observers always see the latest value.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub target: LightingTarget,
    pub session: SessionState,
    pub settings: Settings,
    pub revision: Revision,
    /// Last lighting failure; cleared when lighting next starts successfully.
    pub failure: Option<Error>,
    /// Surfaces that failed. Lighting continues without them.
    pub surface_failures: Vec<Error>,
}
