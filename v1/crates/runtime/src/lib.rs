//! The Prysm runtime: concurrency lives here, adapters are plain blocking objects.
//!
//! - [`Engine`]: one Tokio task and the only mailbox; settings, target, session.
//! - [`Session`]: one thread owning source, processor and output.
//! - [`Forwarder`]: the session loop for capture-only hosts.
//! - [`Prysm`]: the builder hosts list their adapters and surfaces on.
mod builder;
mod engine;
mod forwarder;
mod null;
mod session;
mod surface;
mod worker;

#[cfg(feature = "testkit")]
pub mod fakes;

pub use builder::{Builder, Prysm};
pub use engine::{Engine, EngineHandle};
pub use forwarder::Forwarder;
pub use null::{NullOutput, NullSettings};
pub use session::{Preview, Session, SessionEnd};
pub use surface::{Shutdown, Surface};
