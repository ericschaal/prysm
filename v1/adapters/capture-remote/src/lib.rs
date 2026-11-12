//! Frames over the LAN from a capture-only host (webOS slim) to a lighting
//! host (headless). Both ends are plain blocking adapters on `std::net`:
//!
//! - [`RemoteSender`] is the capture host's [`FrameSink`](prysm_ports::FrameSink).
//! - [`RemoteReceiver`] (kind `"remote"`) opens a [`RemoteSource`], the
//!   lighting host's [`FrameSource`](prysm_ports::FrameSource).
//! - [`StreamRequest`]: what the receiver asks the sender to capture and
//!   how to encode it; the receiver's settings hold it.
//!
//! The wire protocol is described in the crate README. It has no
//! authentication or encryption: use it only on a trusted LAN.
mod receiver;
mod request;
mod sender;
mod wire;

pub use receiver::{POLL_INTERVAL, RemoteReceiver, RemoteReceiverSettings, RemoteSource};
pub use request::{StreamEncoding, StreamRequest};
pub use sender::{CONNECT_TIMEOUT, RemoteSender, WRITE_TIMEOUT};
pub use wire::{MAX_DIMENSION, MAX_PAYLOAD_BYTES, MAX_REASON_BYTES, VERSION, WireError};
