//! The contract between the webOS supervisor (`prysm-webos-service`) and its
//! worker (`prysm-webos` or `prysm-webos-slim`). Design:
//! `docs/plans/prysm-v1-webos-supervisor.md`.
//!
//! - Start: typed argv ([`FullArgs`], [`SlimArgs`]), built without a shell.
//! - Supervisor → worker: [`Command`] lines on the worker's stdin.
//! - Worker → supervisor: [`Event`] lines on the worker's original stdout.
//! - End: the worker's [`exit`] code.
//! - Shared policy: the retry [`Backoff`] of both sides.
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
pub use args::{FullArgs, SlimArgs, SlimEncoding};
pub use codec::{LineReader, LinkError, MAX_LINE, decode, encode};
pub use prysm_ports::LightingTarget;

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
    /// Full only: the TV UI's enable/disable. A slim worker treats it as
    /// protocol misuse and exits [`exit::FAILED`].
    Lighting { target: LightingTarget },
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
    /// Why the worker's run ended with an error (at startup or while
    /// running); it exits next, [`exit::FAILED`], or [`exit::FAULTED`] when
    /// the error or the stop reason is a fault. Not a status: a worker can fail before it has
    /// anything of its package to report. Not every failed exit is preceded
    /// by one: a protocol misuse or lost link (a stop reason, not an error)
    /// exits [`exit::FAILED`] without it, as does a failure before the link
    /// exists.
    Failed { cause: String },
    /// A native resource is unconfirmed; the worker exits [`exit::FAULTED`] next.
    Faulted { cause: String },
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
    /// The portal's LAN address, such as `http://192.168.1.5:8787/`; absent
    /// while the TV has no LAN address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portal: Option<String>,
    /// The last session or portal failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<WorkerFailure>,
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
    Sending { encoding: SlimEncoding, fps: u16 },
    /// The receiver stopped the stream (its lighting is off), and the TV
    /// capture is released until a receiver takes a connection again. Not a
    /// failure.
    Paused {},
    /// The receiver is unavailable (kind [`FailureKind::Receiver`]); the
    /// worker keeps reconnecting.
    Waiting { failure: WorkerFailure },
    /// The encoder, version or replaced failure.
    Failed { failure: WorkerFailure },
}

impl SlimLink {
    /// The wait or the failure, if any.
    #[must_use]
    pub fn failure(&self) -> Option<&WorkerFailure> {
        match self {
            Self::Waiting { failure } | Self::Failed { failure } => Some(failure),
            Self::Starting {} | Self::Sending { .. } | Self::Paused {} => None,
        }
    }
}

impl Default for SlimLink {
    fn default() -> Self {
        Self::Starting {}
    }
}

/// A failure a worker or the supervisor reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerFailure {
    pub kind: FailureKind,
    /// What failed, with its cause chain.
    pub message: String,
}

impl WorkerFailure {
    #[must_use]
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// A failure of kind [`FailureKind::Other`].
    #[must_use]
    pub fn other(message: impl Into<String>) -> Self {
        Self::new(FailureKind::Other, message)
    }
}

/// Delays between attempts after failures: [`Backoff::MIN`] doubling to
/// [`Backoff::MAX`], back to the start after [`Backoff::HEALTHY`]. The full
/// worker uses it to re-apply a failed session, the supervisor to restart a
/// failed worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    first: Duration,
    next: Duration,
}

impl Backoff {
    pub const MIN: Duration = Duration::from_secs(1);
    pub const MAX: Duration = Duration::from_secs(30);
    /// Running this long without a failure resets the delay.
    pub const HEALTHY: Duration = Duration::from_secs(60);

    /// Starting at `first` instead of [`Backoff::MIN`] (tests).
    #[must_use]
    pub const fn starting_at(first: Duration) -> Self {
        Self { first, next: first }
    }

    /// The delay before the next attempt; the one after it doubles, up to
    /// [`Backoff::MAX`].
    pub fn delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = (delay * 2).min(Self::MAX);
        delay
    }

    /// Start over at the first delay.
    pub fn reset(&mut self) {
        self.next = self.first;
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::starting_at(Self::MIN)
    }
}

/// What a reported failure is, so the TV app can offer the matching fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// Slim: the encoder the receiver requested could not start or failed.
    /// The worker refuses that request (the receiver is told) until the
    /// receiver asks for another; it never switches on its own.
    Encoder,
    /// Slim: the receiver is unavailable (its host is not lighting, is
    /// unreachable or its name does not resolve). The worker keeps
    /// reconnecting.
    Receiver,
    /// Slim: Prysm speaks another capture stream version than this TV app;
    /// the older one needs updating. The worker retries slowly, so an update
    /// on either side is picked up without restarting it.
    Version,
    /// Slim: another sender (another TV) took over the stream at Prysm. The
    /// worker released the capture and connects no more until it restarts
    /// (capture stopped and started, the TV waking, or a new Prysm address).
    Replaced,
    /// Anything else.
    Other,
}

/// Worker exit codes.
pub mod exit {
    /// SIGTERM or stdin EOF; not a failure.
    pub const STOPPED: u8 = 0;
    /// Startup or runtime failure, or protocol misuse.
    pub const FAILED: u8 = 1;
    /// Bad argv: a packaging bug.
    pub const USAGE: u8 = 2;
    /// A hung or unconfirmed native resource; only a new process recovers.
    pub const FAULTED: u8 = 3;

    /// A short description of a worker exit code for diagnostics.
    #[must_use]
    pub fn describe(code: i32) -> String {
        match u8::try_from(code) {
            Ok(STOPPED) => "worker stopped".into(),
            Ok(FAILED) => "worker failed".into(),
            Ok(USAGE) => "worker rejected its arguments (packaging bug)".into(),
            Ok(FAULTED) => "worker faulted".into(),
            _ => format!("worker exited with code {code}"),
        }
    }
}

#[cfg(test)]
mod tests;
