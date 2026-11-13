//! Adapter ports and the values the Prysm runtime exchanges with its surfaces.
//!
//! Adapters are plain blocking objects owned by one session thread. They contain
//! no channels, identities or lifecycle state: opening is construction, and the
//! consuming `close`/`idle` calls release exactly once.
mod adapter;
mod calibration;
mod error;
mod profiles;
mod registry;
mod resolve;
mod settings;
mod slot;
mod snapshot;

pub use adapter::{FrameSink, FrameSource, IdleMode, LedOutput, OutputIssue, Pull, SettingsStore};
pub use calibration::{Calibration, Pattern};
pub use error::{Error, Result, ValidationError, chain};
pub use profiles::{
    ColorProfile, CorrectionControls, HueControls, MAX_PROFILE_ID_LEN, MAX_PROFILE_NAME_CHARS,
    MAX_PROFILES,
};
pub use registry::{OutputFactory, Registry, SourceFactory};
pub use resolve::{Lookup, Resolver, system_lookup};
pub use settings::{AdapterSettings, Processing, Revision, Section, Settings};
pub use slot::LatestSlot;
pub use snapshot::{Input, LightingTarget, SessionPhase, SessionState, Snapshot};
