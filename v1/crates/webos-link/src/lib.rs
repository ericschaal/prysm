//! The contract between the webOS supervisor (`prysm-webos-service`) and its
//! worker (`prysm-webos` or `prysm-webos-slim`). Design:
//! `docs/plans/prysm-v1-webos-supervisor.md`.
//!
//! - Start: typed argv ([`FullArgs`], [`SlimArgs`]), built without a shell.
//! - Supervisor → worker: [`Command`] lines on the worker's stdin.
//! - Worker → supervisor: [`Event`] lines on the worker's original stdout.
//! - End: the worker's [`exit`] code.
//!
//! Framing ([`encode`], [`LineReader`]): one JSON object per line, at most
//! [`MAX_LINE`] bytes including the newline, unknown fields rejected. A bad
//! line is a protocol failure, never skipped.
mod args;
mod codec;
#[cfg(feature = "worker")]
pub mod worker;

pub use args::{FullArgs, SlimArgs, SlimCapture, SlimEncoding};
pub use codec::{LineReader, LinkError, MAX_LINE, decode, encode};
pub use prysm_ports::LightingTarget;

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// How often a worker repeats its status when nothing changes.
pub const HEARTBEAT: Duration = Duration::from_secs(5);
/// Silence after which the supervisor treats the worker as hung.
pub const SILENCE: Duration = Duration::from_secs(15);

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

/// The TV's main video output. Maps 1:1 to `prysm_capture_webos::Video`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Video {
    #[default]
    Unknown,
    Unavailable,
    Available,
}

/// Worker → supervisor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    /// Sent on every change and at least every [`HEARTBEAT`].
    Status(WorkerStatus),
    /// A native resource is unconfirmed; the worker exits [`exit::FAULTED`] next.
    Faulted { cause: String },
}

/// What a worker is doing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerStatus {
    /// Frames are flowing. Full: a lighting session is running and frames
    /// arrived in the last second (false while it waits for video). Slim:
    /// frames were sent in the last second.
    pub capturing: bool,
    /// Full only: the engine's target, including changes made in the portal.
    pub target: Option<LightingTarget>,
    /// Full only: the portal's LAN address, such as `http://192.168.1.5:8787/`.
    pub portal: Option<String>,
    /// The last failure with its cause chain.
    pub failure: Option<String>,
    /// What `failure` is; set exactly when `failure` is.
    #[serde(default)]
    pub failure_kind: Option<FailureKind>,
    /// Slim only: the encoding of the frames being sent, while connected to
    /// the receiver (always the configured one).
    #[serde(default)]
    pub encoding: Option<SlimEncoding>,
}

/// What a reported failure is, so the TV app can offer the matching fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// Slim: the configured encoder could not start or failed. Another
    /// encoding may work; the worker never switches on its own.
    Encoder,
    /// Slim: the receiver is unavailable (its host is not lighting or is
    /// unreachable) or paused the stream. The worker keeps reconnecting.
    Receiver,
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

/// An error and its sources, joined with `: `.
#[must_use]
pub fn chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

#[cfg(test)]
mod tests;
