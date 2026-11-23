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
//! - [`RemoteSettings`]: the receiver's settings, which it asks the sender
//!   for: what to capture and how to encode it.
//! - [`Offer`]: what the sender can deliver, sent with each connection
//!   attempt; the receiver answers [`modes`](prysm_ports::SourceFactory::modes)
//!   from the newest.
//!
//! The wire protocol is described in the crate README.
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing;
mod link;
mod offer;
mod receive;
mod receiver;
mod request;
mod sender;
#[cfg(feature = "testkit")]
pub mod testkit;
mod wire;

pub use offer::Offer;
pub use receiver::{RemoteReceiver, RemoteSource, STOP};
pub use request::RemoteSettings;
pub use sender::{CONNECT_DEADLINE, RemoteSender, SendError};
pub use wire::{Close, WireError};
