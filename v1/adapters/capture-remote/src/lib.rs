//! Frames over the LAN from a capture-only host (webOS slim) to a lighting
//! host (headless). Both ends are plain blocking adapters on `std::net`:
//!
//! - [`RemoteSender`] is the capture host's [`FrameSink`](prysm_ports::FrameSink).
//! - [`RemoteReceiver`] (kind `"remote"`) opens a [`RemoteSource`], the
//!   lighting host's [`FrameSource`](prysm_ports::FrameSource).
//!
//! The wire protocol is described in the crate README. It has no
//! authentication or encryption: use it only on a trusted LAN.
mod receiver;
mod sender;
mod wire;

pub use receiver::{POLL_INTERVAL, RemoteReceiver, RemoteReceiverSettings, RemoteSource};
pub use sender::{CONNECT_TIMEOUT, RemoteSender, Stream, WRITE_TIMEOUT};
pub use wire::{Encoding, MAX_DIMENSION, MAX_PAYLOAD_BYTES, VERSION, WireError};
