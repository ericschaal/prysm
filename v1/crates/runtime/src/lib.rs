//! The Prysm runtime: concurrency lives here, adapters are plain blocking objects.
//!
//! - [`Engine`]: one Tokio task and the only mailbox; settings, target, session.
//! - [`Session`]: one thread owning source, processor and output.
//! - [`Counters`]: totals the sessions keep for an opt-in `/metrics`.
//! - [`Forwarder`]: the session loop for capture-only hosts.
//! - [`Prysm`]: the builder hosts list their adapters and surfaces on.
//! - `logging` (feature `logging`): the hosts' `tracing` subscriber.
mod builder;
mod engine;
mod forwarder;
mod null;
mod sample;
mod session;
mod stats;
mod surface;
mod worker;

#[cfg(feature = "testkit")]
pub mod fakes;
#[cfg(feature = "logging")]
pub mod logging;

pub use builder::{Builder, Prysm};
pub use engine::{CALIBRATION_TIMEOUT, Engine, EngineHandle};
pub use forwarder::Forwarder;
pub use null::{NullOutput, NullSettings};
pub use sample::sample_rgb;
pub use session::{
    OpenOutput, Preview, PreviewSlot, PreviewWatch, RELEASE, Session, SessionEnd, VIDEO_GRACE,
};
pub use stats::{Counters, Stats, Timing};
pub use surface::{Shutdown, Surface};
