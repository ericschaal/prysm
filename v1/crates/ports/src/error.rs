use crate::{Revision, SessionPhase};
use std::error::Error as StdError;
use std::sync::Arc;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The one runtime error. Sources are shared (`Arc`) so a failure can be both
/// returned to the caller and kept in the snapshot without losing its chain.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error(transparent)]
    Invalid(#[from] ValidationError),
    #[error("cannot decode `{kind}` settings")]
    Decode {
        kind: String,
        #[source]
        source: Arc<serde_json::Error>,
    },
    #[error("{context}")]
    Adapter {
        context: String,
        #[source]
        source: Option<Arc<dyn StdError + Send + Sync>>,
    },
    #[error("settings revision {expected} is stale; current is {current}")]
    StaleRevision {
        expected: Revision,
        current: Revision,
    },
    /// A session did not stop within its deadline; only shutdown is accepted.
    #[error("runtime is faulted: session did not finish {0}")]
    Faulted(SessionPhase),
    #[error("{0} panicked")]
    Panicked(&'static str),
    #[error("runtime has stopped")]
    Closed,
}

impl Error {
    /// An adapter failure with its cause preserved.
    pub fn adapter(
        context: impl Into<String>,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self::Adapter {
            context: context.into(),
            source: Some(Arc::new(source)),
        }
    }

    /// An adapter failure without an underlying cause.
    pub fn other(context: impl Into<String>) -> Self {
        Self::Adapter {
            context: context.into(),
            source: None,
        }
    }
}

/// A rejected setting. `field` is a dotted path such as `capture.device`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {field}: {reason}")]
pub struct ValidationError {
    pub field: String,
    pub reason: String,
}

impl ValidationError {
    pub fn new(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            reason: reason.into(),
        }
    }
}
