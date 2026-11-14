//! Frames over the LAN from a capture-only host (webOS slim) to a lighting
//! host (headless), through the lighting host's web portal. Both ends are
//! plain blocking adapters on `std::net`:
//!
//! - [`RemoteSender`] is the capture host's [`FrameSink`](prysm_ports::FrameSink):
//!   a WebSocket to the portal's capture stream path (`ws://`, or `wss://`
//!   through a TLS-terminating proxy).
//! - [`RemoteReceiver`] (kind `"remote"`) opens a [`RemoteSource`], the
//!   lighting host's [`FrameSource`](prysm_ports::FrameSource). It reads the
//!   connections the portal hands over through a [`Handoff`](prysm_ports::Handoff).
//! - [`StreamRequest`]: what the receiver asks the sender to capture and
//!   how to encode it; the receiver's settings.
//!
//! The wire protocol is described in the crate README. Each wire message is
//! one binary WebSocket message.
mod link;
mod receive;
mod receiver;
mod request;
mod sender;
#[cfg(feature = "testkit")]
pub mod testkit;
mod wire;

pub use receiver::{RemoteReceiver, RemoteSource};
pub use request::{StreamEncoding, StreamRequest};
pub use sender::{RemoteSender, SendError};
pub use wire::WireError;
