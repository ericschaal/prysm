use crate::{Revision, Section, SessionPhase};
use std::error::Error as StdError;
use std::sync::Arc;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// An error and its sources, joined with `: `, as users see failures.
#[must_use]
pub fn chain(error: &dyn StdError) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// The one runtime error. Sources are shared (`Arc`) so a failure can be both
/// returned to the caller and kept in the snapshot without losing its chain.
#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Invalid(#[from] ValidationError),
    /// The settings store holds something it cannot read as settings, or
    /// cannot read at all: a problem with the settings as a whole, not one
    /// field. The store's error, with its causes, is kept; the engine runs
    /// unconfigured until a save.
    #[error(transparent)]
    UnusableSettings(Arc<Error>),
    /// Adapter settings under `kind` that do not decode as its type.
    #[error("cannot decode `{kind}` settings")]
    Decode {
        kind: String,
        /// Where decoding stopped, a path like [`ValidationError::field`]
        /// (`capture.fps`); the section alone (`capture`) when the section
        /// as a whole is at fault, such as for a missing field.
        field: String,
        #[source]
        source: Arc<serde_json::Error>,
    },
    #[error("{context}")]
    Adapter {
        context: String,
        #[source]
        source: Option<Arc<dyn StdError + Send + Sync>>,
        /// Which side failed, when the engine knows: the capture source or
        /// the output. Set where the error happens ([`Error::in_section`]).
        section: Option<Section>,
    },
    #[error("settings revision {expected} is stale; current is {current}")]
    StaleRevision {
        expected: Revision,
        current: Revision,
    },
    /// A calibration pattern was asked for while the target is not
    /// `ScreenSync`: patterns are shown through screen sync.
    #[error("calibration patterns need screen sync: turn it on first")]
    NeedsScreenSync,
    /// A session did not stop within its deadline, or did not confirm its
    /// source's release; only shutdown is accepted.
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
            section: None,
        }
    }

    /// An adapter failure without an underlying cause.
    pub fn other(context: impl Into<String>) -> Self {
        Self::Adapter {
            context: context.into(),
            source: None,
            section: None,
        }
    }

    /// This error as one of `section`'s: an adapter failure that does not
    /// already say whose it is now does. Other errors are unchanged.
    #[must_use]
    pub fn in_section(self, section: Section) -> Self {
        match self {
            Self::Adapter {
                context,
                source,
                section: None,
            } => Self::Adapter {
                context,
                source,
                section: Some(section),
            },
            other => other,
        }
    }

    /// Which side an adapter failure came from, when known.
    #[must_use]
    pub fn section(&self) -> Option<Section> {
        match self {
            Self::Adapter { section, .. } => *section,
            _ => None,
        }
    }

    /// The first cause of type `T` in this error's source chain.
    #[must_use]
    pub fn cause<T: StdError + 'static>(&self) -> Option<&T> {
        // From inside the `Arc`s: `source()` gives the `Arc` itself, which
        // does not downcast to what it holds.
        let mut source: Option<&(dyn StdError + 'static)> = match self {
            Self::UnusableSettings(stored) => return Self::cause::<T>(stored),
            Self::Adapter { source, .. } => source.as_deref().map(|cause| cause as _),
            Self::Decode { source, .. } => Some(&**source),
            Self::Invalid(invalid) => Some(invalid),
            other => StdError::source(other),
        };
        while let Some(cause) = source {
            if let Some(found) = cause.downcast_ref::<T>() {
                return Some(found);
            }
            source = cause.source();
        }
        None
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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("wrapped")]
    struct Wrapped(#[source] std::io::Error);

    #[test]
    fn cause_walks_the_whole_chain() {
        let io = || std::io::Error::other("unplugged");
        let direct = Error::adapter("cannot read", io());
        assert_eq!(
            direct.cause::<std::io::Error>().unwrap().to_string(),
            "unplugged"
        );
        let deeper = Error::adapter("cannot read", Wrapped(io()));
        assert!(deeper.cause::<Wrapped>().is_some());
        assert_eq!(
            deeper.cause::<std::io::Error>().unwrap().to_string(),
            "unplugged"
        );
        assert!(deeper.cause::<ValidationError>().is_none());
        let invalid = Error::Invalid(ValidationError::new("capture.device", "empty"));
        assert!(invalid.cause::<ValidationError>().is_some());
        assert!(Error::other("no cause").cause::<std::io::Error>().is_none());
        let decode = Error::Decode {
            kind: "fake".into(),
            field: "capture.fps".into(),
            source: Arc::new(serde_json::from_str::<u8>("x").unwrap_err()),
        };
        assert!(decode.cause::<serde_json::Error>().is_some());
        let unusable = Error::UnusableSettings(Arc::new(direct));
        assert_eq!(
            unusable.cause::<std::io::Error>().unwrap().to_string(),
            "unplugged"
        );
        assert_eq!(chain(&unusable), "cannot read: unplugged");
    }
}
