//! Adapter ports and the values the Prysm runtime exchanges with its surfaces.
//!
//! Adapters are plain blocking objects owned by one session thread. They contain
//! no channels, identities or lifecycle state: opening is construction, and the
//! consuming `close`/`idle` calls release exactly once.
mod adapter;
mod calibration;
mod calibrations;
pub mod capture_size;
mod error;
mod handoff;
mod owner;
mod registry;
mod resolve;
mod settings;
mod slot;
mod snapshot;
mod style;

pub use adapter::{
    DeviceBrightness, FrameSink, FrameSource, IdleMode, LedOutput, OutputIssue, Pull, SettingsStore,
};
pub use calibration::{CalibrationPattern, Pattern};
pub use calibrations::{
    Calibration, CorrectionControls, MAX_CALIBRATION_ID_LEN, MAX_CALIBRATION_NAME_CHARS,
    MAX_CALIBRATIONS, PrimaryControls,
};
pub use error::{Error, Result, ValidationError, chain};
pub use handoff::{
    Accepted, CAPTURE_PROTOCOL, CAPTURE_STREAM_PATH, CAPTURE_VERSION, Handoff, Receiving,
};
pub use owner::{OwnerThread, Worker};
pub use registry::{Candidate, OutputFactory, Registry, SourceFactory};
pub use resolve::{Lookup, Resolver, system_lookup};
pub use settings::{AdapterSettings, Processing, Revision, Section, Settings};
pub use slot::LatestSlot;
pub use snapshot::{Behind, Input, Latency, LightingTarget, SessionPhase, SessionState, Snapshot};
pub use style::{Style, StyleValues};
