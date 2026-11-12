//! Adapter ports and the values the Prysm runtime exchanges with its surfaces.
//!
//! Adapters are plain blocking objects owned by one session thread. They contain
//! no channels, identities or lifecycle state: opening is construction, and the
//! consuming `close`/`idle` calls release exactly once.
mod adapter;
mod error;
mod registry;
mod settings;
mod slot;
mod snapshot;

pub use adapter::{FrameSink, FrameSource, IdleMode, LedOutput, SettingsStore};
pub use error::{Error, Result, ValidationError};
pub use registry::{OutputFactory, Registry, SourceFactory};
pub use settings::{AdapterSettings, Processing, Revision, Settings};
pub use slot::LatestSlot;
pub use snapshot::{LightingTarget, SessionPhase, SessionState, Snapshot};
