//! Adapter ports and the values the Prysm runtime exchanges with its surfaces.
//!
//! Adapters are plain blocking objects owned by one session thread. They contain
//! no channels, identities or lifecycle state: opening is construction, and the
//! consuming `close`/`release` calls release exactly once. The pure
//! [`AdapterLifecycle`] models readiness and the ownership of those effects.
mod adapter;
mod availability;
mod calibration;
mod calibrations;
pub mod capture_size;
mod encoding;
mod error;
mod handoff;
mod hangup;
mod map_only;
mod modes;
mod owner;
mod recovery;
mod registry;
mod resolve;
mod settings;
mod snapshot;
mod style;

pub use adapter::{
    DeviceBrightness, FrameSink, FrameSource, IdleMode, LedOutput, OutputIssue, Pull,
    SettingsStore, SourceStop,
};
pub use availability::{
    AdapterLifecycle, Availability, Claim, OpenIntent, OutputDisplay, OutputObservation,
    OutputObservationState, OutputOpen, OutputRelease, Power, Readiness, Release,
};
pub use calibration::{CalibrationPattern, Pattern};
pub use calibrations::{
    Calibration, CorrectionControls, MAX_CALIBRATION_ID_LEN, MAX_CALIBRATION_NAME_CHARS,
    MAX_CALIBRATIONS, PrimaryControls,
};
pub use encoding::StreamEncoding;
pub use error::{Error, Result, ValidationError, chain};
pub use handoff::{
    Accepted, Answer, CAPTURE_OFFER_HEADER, CAPTURE_PROTOCOL, CAPTURE_PROTOCOL_PREFIX,
    CAPTURE_STREAM_PATH, CAPTURE_VERSION, Handoff, MAX_ANNOUNCEMENT_BYTES, Receiving,
};
pub use hangup::{CallError, Hangup, Registration, Stopping};
#[cfg(feature = "schema")]
pub use modes::SpanFields as SpanSchema;
pub use modes::{Mode, Modes, Span};
pub use owner::{OwnerThread, Wakeup, Worker};
pub use recovery::{Escalation, Fix};
pub use registry::{
    Candidate, ConfiguredAdapter, ConfiguredOutput, ConfiguredSource, OpenFailure, OutputFactory,
    Registry, SOURCE_OPEN_LIMIT, SourceFactory,
};
pub use resolve::{Lookup, Resolver, system_lookup};
pub use settings::{AdapterSettings, Processing, Revision, Section, Settings};
pub use snapshot::{
    Behind, DeviceReport, Input, Latency, LightingControl, LightingMode, LightingTarget, Picture,
    SessionPhase, SessionState, Snapshot, WaitingFor,
};
pub use style::{Style, StyleValues};

/// For [`map_only!`] in other crates.
#[doc(hidden)]
pub use serde as __serde;
