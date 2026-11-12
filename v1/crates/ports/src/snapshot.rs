use crate::{Calibration, Error, OutputIssue, Revision, Settings};
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
    /// Frames arriving from the source; `None` while no session runs.
    pub input: Option<Input>,
    /// Why the LEDs are not showing the session's colours, from the output's
    /// own checks; republished with `input`. `None` while they show them,
    /// while the output cannot tell, and while no session holds the output.
    pub output_issue: Option<OutputIssue>,
    /// The calibration pattern shown, while the target is `Processed`. Not
    /// persisted; while set, processing runs without smoothing (and without
    /// correction when `uncorrected`), leaving `settings` untouched.
    pub calibration: Option<Calibration>,
    /// Last lighting failure; cleared when lighting next starts successfully.
    pub failure: Option<Error>,
    /// Surfaces that failed. Lighting continues without them.
    pub surface_failures: Vec<Error>,
}

/// Frames arriving from the running session's source, measured over about one
/// second and republished at most that often.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Input {
    /// Frames per second over the last window, to one decimal.
    pub fps: f32,
    /// At least one frame arrived in the last window. `false` right after a
    /// session starts and while a source is quiet (no signal, no sender):
    /// lighting is waiting for video. The session takes the output only at a
    /// frame, and releases it to the device's own state (`IdleMode::Native`)
    /// after a few seconds without one, so a waiting session does not hold
    /// the LEDs.
    pub receiving: bool,
}
