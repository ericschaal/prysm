use crate::{CalibrationPattern, DeviceBrightness, Error, OutputIssue, Revision, Settings};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The selected mode restored by the Power button.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum LightingMode {
    ScreenSync,
    #[default]
    Effects,
}

/// Intent and observed power have separate lifetimes. An external live stream
/// leaves both mode buttons unselected until the user requests a mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct LightingControl {
    pub remembered: LightingMode,
    pub external: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[cfg_attr(feature = "schema", schema(nullable = false))]
    pub power: Option<crate::Power>,
    pub busy: bool,
    /// A requested mode is waiting for a prerequisite that can recover.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[cfg_attr(feature = "schema", schema(nullable = false))]
    pub waiting: Option<WaitingFor>,
}

/// The required adapter holding up the current request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum WaitingFor {
    Capture,
    Output,
}

/// What the user wants the LEDs to do.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    strum::AsRefStr,
    strum::IntoStaticStr,
    strum::VariantNames,
)]
#[strum(serialize_all = "snake_case", const_into_str)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum LightingTarget {
    #[default]
    Off,
    /// The lights follow the colours of captured frames.
    ScreenSync,
    /// The output's own effects; no capture.
    Effects,
}

impl LightingTarget {
    pub const ALL: [Self; 3] = [Self::Off, Self::ScreenSync, Self::Effects];

    /// The serialized name, such as `screen_sync`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.into_str()
    }
}

/// What the runtime is actually doing. A failure is kept only while it
/// explains the present: why the target is not reached, why the lights may
/// not be off, or that the saved settings do not validate. Screen sync runs
/// on valid settings and has reached its target, so it carries none.
#[derive(Debug, Clone, strum::AsRefStr, strum::VariantNames)]
#[strum(serialize_all = "snake_case")]
pub enum SessionState {
    /// No running session. A selected mode may be waiting for prerequisites.
    Stopped { failure: Option<Error> },
    /// A session runs.
    ScreenSync,
    /// Display is delegated to the output's own effects.
    Effects { failure: Option<Error> },
    /// The requested mode is waiting for the output to answer.
    Unanswered { failure: Option<Error> },
    /// A session did not stop in time, or its source's release is
    /// unconfirmed. Only shutdown is accepted.
    Faulted(SessionPhase),
}

impl Default for SessionState {
    fn default() -> Self {
        Self::Stopped { failure: None }
    }
}

impl SessionState {
    /// The failure this state carries, if any.
    #[must_use]
    pub fn failure(&self) -> Option<&Error> {
        match self {
            Self::Stopped { failure }
            | Self::Effects { failure }
            | Self::Unanswered { failure } => failure.as_ref(),
            Self::ScreenSync | Self::Faulted(_) => None,
        }
    }

    /// Whether this state is what `target` asks for.
    #[must_use]
    pub fn reached(&self, target: LightingTarget) -> bool {
        matches!(
            (target, self),
            (LightingTarget::Off, Self::Stopped { .. })
                | (LightingTarget::ScreenSync, Self::ScreenSync)
                | (LightingTarget::Effects, Self::Effects { .. })
        )
    }
}

/// `stopped`, `screen_sync`, `effects`, `unanswered` or `faulted while
/// <phase>`; the failure is not part of it.
impl fmt::Display for SessionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Faulted(phase) => write!(f, "faulted while {phase}"),
            state => f.write_str(state.as_ref()),
        }
    }
}

/// Where a session thread is when it fails to stop in time, or
/// [`SourceUnreleased`](Self::SourceUnreleased) when it finished without
/// confirming its source's release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPhase {
    Running,
    ClosingSource,
    IdlingOutput,
    Finishing,
    /// The source's close failed or panicked, or the thread died before
    /// reporting it: the source may still hold its device, so no new session
    /// can be trusted to open it.
    SourceUnreleased,
}

impl fmt::Display for SessionPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Running => "running",
            Self::ClosingSource => "closing the source",
            Self::IdlingOutput => "idling the output",
            Self::Finishing => "finishing",
            Self::SourceUnreleased => "confirming the source's release",
        })
    }
}

/// The runtime's observable state. Observers always see the latest value.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub control: LightingControl,
    pub target: LightingTarget,
    pub session: SessionState,
    pub settings: Settings,
    pub revision: Revision,
    /// Frames arriving from the source; `None` while no session runs.
    pub input: Option<Input>,
    /// What the output's device says about itself, from the output's own
    /// checks; republished with `input`. Empty while no session holds the
    /// output, but for [`OutputIssue::Unreachable`] while the lights are
    /// waited for: a session's open they did not answer, or
    /// [`SessionState::Unanswered`].
    pub device: DeviceReport,
    /// The calibration pattern shown, while the target is `ScreenSync`. Not
    /// persisted; while set, processing runs without smoothing (and without
    /// correction when `uncorrected`), leaving `settings` untouched.
    pub calibration: Option<CalibrationPattern>,
    /// Surfaces that failed. Lighting continues without them.
    pub surface_failures: Vec<Error>,
}

/// What a held output's device last said about itself
/// ([`LedOutput::report`](crate::LedOutput::report)), or, while the lights
/// are waited for (the session's open, or the request for `Off` or their own
/// effects, that they did not answer), only that issue. Each part
/// is `None` while the output cannot tell; Prysm reports them and changes
/// none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceReport {
    /// Why the LEDs are not showing the session's colours, or that they do
    /// not answer the request for the target; `None` while all is well.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<OutputIssue>,
    /// The device's own brightness while it scales the session's colours
    /// (they show at that percent of the brightness Prysm asks for);
    /// `None` while it does not.
    #[serde(
        default,
        rename = "brightness_percent",
        skip_serializing_if = "Option::is_none"
    )]
    pub brightness: Option<DeviceBrightness>,
    /// How many LEDs the device drives; the configured edges may say
    /// otherwise. A report, not a failure: other counts leave LEDs dark or
    /// colours unshown, and writing goes on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led_count: Option<u32>,
}

/// Frames arriving from the running session's source, measured over about one
/// second and republished at most that often.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Input {
    /// Frames per second over the last window, to one decimal.
    pub fps: f32,
    /// Whether frames arrived in the last window, and if not, why.
    pub picture: Picture,
    /// Frames per second whose colours were handed to the output over the
    /// last window, to one decimal. An output may drop a frame it cannot
    /// send now; the device issue says when the lights do not answer.
    #[serde(default)]
    pub sent_fps: f32,
    /// The mean time per frame along the path, in two parts, over the last
    /// window. Absent when no frame reached the output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency: Option<Latency>,
    /// Which step keeps Prysm from keeping up with its source, if one does.
    #[serde(default, skip_serializing_if = "Behind::is_none")]
    pub behind: Behind,
}

/// Whether frames arrive from the source. `arriving`: at least one frame in
/// the last second. `quiet`: none, and no reason given (no signal, no
/// sender); also right after lighting starts. `refused`: none, because the
/// source's device refuses to share the video it shows; it ends when the
/// video changes. Neither `quiet` nor `refused` is a failure: lighting waits
/// for video, and releases the lights after 3 s without it.
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Picture {
    /// At least one frame arrived in the window.
    Arriving,
    /// No frame, and no reason given (no signal, no sender); also right
    /// after a session starts. Lighting waits for video.
    #[default]
    Quiet,
    /// No frame: the source's device refuses to share the video it shows;
    /// frames come again once it shares it. Lighting waits for video, as
    /// while quiet.
    Refused,
}

impl Picture {
    #[must_use]
    pub fn is_arriving(self) -> bool {
        self == Self::Arriving
    }
}

/// What makes Prysm fall behind its source: the frame loop has had no slack
/// (busy almost all the time) for several seconds, and this step took the
/// larger share. A source that just sends fewer frames leaves slack and is
/// never this.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Behind {
    /// Prysm keeps up.
    #[default]
    None,
    /// Turning frames into colours takes most of the time.
    Processing,
    /// Writing the colours to the lights takes most of the time.
    Output,
}

impl Behind {
    /// Prysm keeps up: nothing to report.
    #[must_use]
    pub fn is_none(&self) -> bool {
        *self == Self::None
    }
}

/// Mean milliseconds per frame along the path, measured by Prysm (not from
/// the screen, and not including the lights' own delay), in two parts.
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Latency {
    /// From the source capturing the frame to Prysm receiving it; present
    /// only when every frame flushed in the window carried a capture time
    /// (video devices on macOS, and on Linux with monotonic stamps).
    #[cfg_attr(
        feature = "schema",
        schema(minimum = 0, example = 88.0, nullable = false)
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_ms: Option<f32>,
    /// From Prysm receiving the frame to the output write for its colours
    /// returning.
    #[cfg_attr(feature = "schema", schema(minimum = 0, example = 2.4))]
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

    /// A wait for the lights reaches no target, also the one it waits for.
    #[test]
    fn lights_that_do_not_answer_reach_no_target() {
        let waiting = SessionState::Unanswered { failure: None };
        for target in LightingTarget::ALL {
            assert!(!waiting.reached(target), "{target:?}");
        }
    }

    #[test]
    fn session_states_display_their_phase() {
        assert_eq!(SessionState::default().to_string(), "stopped");
        assert_eq!(SessionState::ScreenSync.to_string(), "screen_sync");
        assert_eq!(
            SessionState::Effects {
                failure: Some(Error::other("cannot idle"))
            }
            .to_string(),
            "effects"
        );
        assert_eq!(
            SessionState::Unanswered { failure: None }.to_string(),
            "unanswered"
        );
        assert_eq!(
            SessionState::Faulted(SessionPhase::ClosingSource).to_string(),
            "faulted while closing the source"
        );
        assert_eq!(
            SessionState::Faulted(SessionPhase::SourceUnreleased).to_string(),
            "faulted while confirming the source's release"
        );
    }

    #[test]
    fn a_state_reaches_only_its_own_target() {
        let failure = || Some(Error::other("x"));
        let states = [
            SessionState::Stopped { failure: None },
            SessionState::Stopped { failure: failure() },
            SessionState::ScreenSync,
            SessionState::Effects { failure: None },
            SessionState::Effects { failure: failure() },
            SessionState::Faulted(SessionPhase::Running),
        ];
        let expected: [&[LightingTarget]; 6] = [
            &[LightingTarget::Off],
            &[LightingTarget::Off],
            &[LightingTarget::ScreenSync],
            &[LightingTarget::Effects],
            &[LightingTarget::Effects],
            &[],
        ];
        for (state, expected) in states.iter().zip(expected) {
            let reached: Vec<_> = LightingTarget::ALL
                .into_iter()
                .filter(|target| state.reached(*target))
                .collect();
            assert_eq!(reached, expected, "{state}");
        }
        assert!(states[1].failure().is_some() && states[4].failure().is_some());
        assert!(states[2].failure().is_none() && states[5].failure().is_none());
    }
}
