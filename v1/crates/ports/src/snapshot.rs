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

impl LightingTarget {
    pub const ALL: [Self; 3] = [Self::Off, Self::Processed, Self::DeviceNative];

    /// The serialized name, such as `device_native`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Processed => "processed",
            Self::DeviceNative => "device_native",
        }
    }
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

/// `stopped`, `running`, `native` or `faulted while <phase>`.
impl fmt::Display for SessionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stopped => f.write_str("stopped"),
            Self::Running => f.write_str("running"),
            Self::Native => f.write_str("native"),
            Self::Faulted(phase) => write!(f, "faulted while {phase}"),
        }
    }
}

/// Where a session thread is; reported when it fails to stop in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPhase {
    Running,
    ClosingSource,
    IdlingOutput,
    Finishing,
}

impl fmt::Display for SessionPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Running => "running",
            Self::ClosingSource => "closing the source",
            Self::IdlingOutput => "idling the output",
            Self::Finishing => "finishing",
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
    /// The last lighting failure. Every target change, settings apply,
    /// shutdown and session end replaces it, so the next one that succeeds
    /// clears it.
    pub failure: Option<Error>,
    /// Surfaces that failed. Lighting continues without them.
    pub surface_failures: Vec<Error>,
}

/// Frames arriving from the running session's source, measured over about one
/// second and republished at most that often.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
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
    /// Frames per second whose colours were written to the output over the
    /// last window, to one decimal.
    #[serde(default)]
    pub sent_fps: f32,
    /// The mean time per frame along the path, in two parts, over the last
    /// window. Absent when no frame reached the output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency: Option<Latency>,
}

/// Mean milliseconds per frame along the path, measured by Prysm, in two
/// parts: from the source capturing it to Prysm receiving it, and from Prysm
/// receiving it to the output write for its colours returning.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Latency {
    /// Capture to received; present only when every frame flushed in the
    /// window carried a capture time (video devices on macOS, and on Linux
    /// with monotonic stamps).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_ms: Option<f32>,
    /// Received to written.
    pub output_ms: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_names_are_the_serde_names() {
        for target in LightingTarget::ALL {
            assert_eq!(serde_json::to_value(target).unwrap(), target.as_str());
        }
    }

    #[test]
    fn session_states_display_their_phase() {
        assert_eq!(SessionState::Native.to_string(), "native");
        assert_eq!(
            SessionState::Faulted(SessionPhase::ClosingSource).to_string(),
            "faulted while closing the source"
        );
    }
}
