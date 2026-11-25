//! The contract between the webOS supervisor (`prysm-webos-service`) and its
//! worker (`prysm-webos` or `prysm-webos-slim`). How they use it:
//! `docs/developers/webos.md`.
//!
//! - Start: typed argv ([`FullArgs`], [`SlimArgs`]), built without a shell.
//! - Supervisor → worker: [`Command`] lines on the worker's stdin.
//! - Worker → supervisor: [`Event`] lines on the worker's original stdout.
//! - End: the worker's exit, its code 0 for a stop asked for and 1 for
//!   anything else (`worker::exit_code`); the supervisor gives it no
//!   meaning.
//!
//! Framing ([`encode`], [`LineReader`]): one JSON object per line, at most
//! [`MAX_LINE`] bytes including the newline, unknown fields rejected. A bad
//! line is a protocol failure, never skipped.
mod address;
mod args;
mod codec;
#[cfg(feature = "worker")]
pub mod worker;

pub use address::{PrysmAddress, Scheme};
pub use args::{FullArgs, SlimArgs};
pub use codec::{LineReader, LinkError, MAX_LINE, decode, encode};
use prysm_ports::StreamEncoding;
pub use prysm_ports::{Escalation, Fix, LightingTarget, OutputIssue};

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// How often a worker repeats its status when nothing changes.
pub const HEARTBEAT: Duration = Duration::from_secs(5);
/// Silence after which the supervisor treats the worker as hung.
pub const SILENCE: Duration = Duration::from_secs(15);
/// How long the supervisor waits between a worker's SIGTERM and its
/// SIGKILL. Each worker's whole stop must fit inside it; each worker checks
/// that at compile time.
pub const KILL_AFTER: Duration = Duration::from_secs(8);

/// Supervisor → worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    /// Effective MAIN video: `Unavailable` whenever TV power is not Active.
    Video { video: Video },
    /// Input, app or picture mode changed: reopen the native tap. (A struct
    /// variant, so unknown fields are rejected like everywhere else.)
    ResetVideo {},
    /// Full only: the lighting target the TV app chose. `sequence` counts
    /// the targets sent to this worker from 1; its status says the last one
    /// it applied ([`FullStatus::applied`]). A slim worker treats it as
    /// protocol misuse and exits.
    Target {
        target: LightingTarget,
        sequence: u64,
    },
}

/// The TV's main video output, as Luna `videooutput/getStatus` reports the
/// `MAIN` sink. Also the webOS capture adapter's readiness input
/// (`prysm_capture_webos::VideoSignal`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Video {
    /// No report yet. Capture is attempted, bounded by the adapter's
    /// preparation limit.
    #[default]
    Unknown,
    /// Disconnected or zero-sized. The native tap is released and frames stop.
    Unavailable,
    /// Connected with a size and frame rate. Muted video still counts.
    Available,
}

/// Worker → supervisor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    /// Sent on every change and at least every [`HEARTBEAT`].
    Status(WorkerStatus),
    /// Why the worker's run ends with an error (at startup or while
    /// running); it exits next. Not a status: a worker can fail before it
    /// has anything of its package to report. Not every failed exit is
    /// preceded by one: a protocol misuse or lost link (a stop reason, not
    /// an error) exits without it, as does a failure before the link
    /// exists.
    Failed(Failure),
}

/// A worker's failure: what is left of it, copied from
/// [`prysm_ports::Error::escalation`] where the error is met (never decided
/// on the wire), and its message with the causes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    pub escalation: Escalation,
    pub message: String,
}

impl Failure {
    /// `error`'s escalation and cause chain.
    #[must_use]
    pub fn of(error: &prysm_ports::Error) -> Self {
        Self {
            escalation: error.escalation(),
            message: prysm_ports::chain(error),
        }
    }
}

/// What a worker is doing, in its package's terms (`"flavor": "full"` or
/// `"slim"`), so the supervisor never has to guess which fields apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "flavor", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerStatus {
    Full(FullStatus),
    Slim(SlimStatus),
}

/// The full worker (`prysm-webos`): engine, WLED and portal on the TV.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FullStatus {
    /// A lighting session is running and frames arrived in the last second
    /// (false while it waits for video).
    pub capturing: bool,
    /// Frames per second over about the last second, rounded, while a
    /// lighting session runs; `Some(0)` while it waits for frames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<u16>,
    /// The engine's target, including changes made in the portal.
    pub target: LightingTarget,
    /// The engine's device report's issue: why the lights are not showing
    /// the session's colours while a lighting session runs, `Unreachable`
    /// also while the session waits for lights that do not answer. With the
    /// target `Off` or `Effects`, `Unreachable` alone, while the lights do
    /// not answer the request that puts them in that mode; it is tried again
    /// until they do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lights: Option<OutputIssue>,
    /// The `sequence` of the last [`Command::Target`] applied before this
    /// status; 0 before any (the worker started with the target in its
    /// arguments). A status that applied an older one than the supervisor
    /// sent was sent before the worker read it.
    pub applied: u64,
    /// The portal's LAN address, such as `http://192.168.1.5:8787/`; absent
    /// while the TV has no LAN address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portal: Option<String>,
    /// A faulted session (escalated as `Restart`), else the session's
    /// failure, else the last portal failure. It stays until a command
    /// changes it: the worker tries nothing again by itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
}

/// The slim worker (`prysm-webos-slim`): capture streamed to a desktop Prysm.
/// The default is a worker that has not tried to connect yet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlimStatus {
    /// Frames were sent in the last second (so also just after the stream
    /// ended).
    pub capturing: bool,
    /// Where the uplink to the receiver stands.
    pub link: SlimLink,
}

/// Where the slim worker's uplink to the receiver stands, as
/// `{ "state": … }`. Struct variants throughout, so unknown fields are
/// rejected in every state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum SlimLink {
    /// No connection attempt yet.
    Starting {},
    /// Connected: frames go out in `encoding` (always the one the receiver
    /// requested), `fps` per second over about the last second, rounded; 0
    /// while none come.
    Sending { encoding: StreamEncoding, fps: u16 },
    /// Connected, but the TV refuses to share the video it shows, so no
    /// frames go out; it ends when the video changes. Not a failure.
    Refused {},
    /// The receiver stopped the stream (its lighting is off), and the TV
    /// capture is released until a receiver takes a connection again. Not a
    /// failure.
    Paused {},
    /// The receiver is unavailable (its host is not lighting, is
    /// unreachable or its name does not resolve), and why. The worker keeps
    /// reconnecting. Not a failure.
    Waiting { message: String },
    /// The encoder the receiver requested could not start or failed. The
    /// worker refuses that request (the receiver is told) until the
    /// receiver asks for another; it never switches on its own.
    EncoderFailed { message: String },
    /// Prysm speaks another capture stream version than this TV app; the
    /// older one needs updating. The worker keeps reconnecting, so an update
    /// on either side is picked up without restarting it.
    VersionDiffers { message: String },
    /// Prysm's receiving source failed with this TV's stream (it refused the
    /// request, a keyframe did not decode, or Prysm has no decoder); Prysm
    /// shows why. The worker released the capture and keeps reconnecting, so
    /// the next lighting start on the computer gets through.
    ReceiverFailed { message: String },
}

impl SlimLink {
    /// Why it waits or does not send, in the states that say.
    pub fn message_mut(&mut self) -> Option<&mut String> {
        match self {
            Self::Waiting { message }
            | Self::EncoderFailed { message }
            | Self::VersionDiffers { message }
            | Self::ReceiverFailed { message } => Some(message),
            Self::Starting {} | Self::Sending { .. } | Self::Refused {} | Self::Paused {} => None,
        }
    }
}

impl Default for SlimLink {
    fn default() -> Self {
        Self::Starting {}
    }
}

#[cfg(test)]
mod tests;
