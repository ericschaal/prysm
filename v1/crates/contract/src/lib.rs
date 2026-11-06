//! V1 domain ports. No device implementations, HTTP types, or Tokio channels.
//! Async traits use static dispatch; they are intentionally not dyn-compatible.
pub mod application;
pub mod calibration;
pub mod capture;
pub mod identity;
pub mod observation;
pub mod output;
pub mod preview;
pub mod processing;
pub mod storage;

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
    #[error("owner recovery required: {0}")]
    RecoveryRequired(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error(transparent)]
    Reported(#[from] Failure),
}

/// Stable categories for callers; adapter details never determine recovery policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Unavailable,
    Invalid,
    Conflict,
    OutcomeUnknown,
    CleanupPending,
    UnknownOperation,
    Adapter,
    /// Retrying commands cannot recover this owner; host/device recovery is required.
    RecoveryRequired,
    Interrupted,
}

impl ControlError {
    pub fn kind(&self) -> FailureKind {
        match self {
            Self::Unavailable(_) => FailureKind::Unavailable,
            Self::Invalid { .. } => FailureKind::Invalid,
            Self::Conflict(_) => FailureKind::Conflict,
            Self::OutcomeUnknown(_) => FailureKind::OutcomeUnknown,
            Self::CleanupPending(_) => FailureKind::CleanupPending,
            Self::UnknownOperation => FailureKind::UnknownOperation,
            Self::Adapter(_) => FailureKind::Adapter,
            Self::RecoveryRequired(_) => FailureKind::RecoveryRequired,
            Self::Reported(failure) => failure.kind,
        }
    }
}

/// Public diagnostics retain a local source chain that wire projections omit.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct Failure {
    pub kind: FailureKind,
    pub message: String,
    #[source]
    source: Option<std::sync::Arc<ControlError>>,
}

impl Failure {
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }
}

impl From<ControlError> for Failure {
    fn from(error: ControlError) -> Self {
        if let ControlError::Reported(failure) = error {
            return failure;
        }
        Self {
            kind: error.kind(),
            message: error.to_string(),
            source: Some(std::sync::Arc::new(error)),
        }
    }
}

impl PartialEq for Failure {
    fn eq(&self, other: &Self) -> bool {
        // Snapshot equality concerns public diagnostics, not local source identity.
        self.kind == other.kind && self.message == other.message
    }
}
impl Eq for Failure {}

/// Completion of an owner, distinct from merely requesting cancellation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerStopped;

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn reported_failures_preserve_their_category_and_local_source_chain() {
        let failure = Failure::from(ControlError::RecoveryRequired(Box::new(
            std::io::Error::other("ambiguous device response"),
        )));
        assert_eq!(failure.kind, FailureKind::RecoveryRequired);
        let error = ControlError::from(failure.clone());
        assert_eq!(error.kind(), FailureKind::RecoveryRequired);
        assert_eq!(Failure::from(error), failure);
        assert_eq!(
            failure.source().unwrap().source().unwrap().to_string(),
            "ambiguous device response"
        );
    }
}
