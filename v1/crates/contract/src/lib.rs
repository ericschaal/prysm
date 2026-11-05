//! V1 domain ports. No device implementations, HTTP types, or Tokio channels.
//! Async traits use static dispatch; they are intentionally not dyn-compatible.
pub mod application;
pub mod calibration;
pub mod capture;
#[cfg(feature = "wire")]
pub mod client;
pub mod identity;
pub mod observation;
pub mod output;
pub mod preview;
pub mod processing;
#[cfg(feature = "schema")]
pub mod schema;
pub mod storage;
#[cfg(feature = "wire")]
pub mod wire;

use identity::{OperationId, SessionId};

/// Outcomes callers can act on independently of a transport implementation.
#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("unavailable: {0}")]
    Unavailable(&'static str),
    #[error("invalid {field}: {reason}")]
    Invalid { field: &'static str, reason: String },
    #[error("conflict: {0}")]
    Conflict(&'static str),
    #[error("operation outcome unknown: {0:?}")]
    OutcomeUnknown(OperationId),
    #[error("cleanup remains pending: {0:?}")]
    CleanupPending(SessionId),
    #[error("operation is unknown or its retained result expired")]
    UnknownOperation,
    #[error("adapter failed: {0}")]
    Adapter(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// Displayable failure information for snapshots, separate from local error chains.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "wire", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Failure {
    pub message: String,
}

/// Completion of an owner, distinct from merely requesting cancellation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerStopped;
