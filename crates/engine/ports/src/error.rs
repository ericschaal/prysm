use crate::{Escalation, Fix, Recovery, Revision, Section, SessionPhase};
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
        /// What makes it work again, set when the error is built:
        /// [`Escalation::FAIL_FAST`] ([`Error::adapter`], [`Error::other`])
        /// unless the adapter says ([`Error::adapter_recovering`],
        /// [`Error::other_recovering`]).
        recovery: Recovery,
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
    /// An adapter failure with its cause preserved; it fails fast.
    pub fn adapter(
        context: impl Into<String>,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self::adapter_recovering(context, source, Recovery::Escalate(Escalation::FAIL_FAST))
    }

    /// An adapter failure without an underlying cause; it fails fast.
    pub fn other(context: impl Into<String>) -> Self {
        Self::other_recovering(context, Recovery::Escalate(Escalation::FAIL_FAST))
    }

    /// An adapter failure with its cause preserved, made to work again by
    /// `recovery`.
    pub fn adapter_recovering(
        context: impl Into<String>,
        source: impl StdError + Send + Sync + 'static,
        recovery: Recovery,
    ) -> Self {
        Self::Adapter {
            context: context.into(),
            source: Some(Arc::new(source)),
            section: None,
            recovery,
        }
    }

    /// An adapter failure without an underlying cause, made to work again by
    /// `recovery`.
    pub fn other_recovering(context: impl Into<String>, recovery: Recovery) -> Self {
        Self::Adapter {
            context: context.into(),
            source: None,
            section: None,
            recovery,
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
                recovery,
            } => Self::Adapter {
                context,
                source,
                section: Some(section),
                recovery,
            },
            other => other,
        }
    }

    /// What makes this failure work again.
    #[must_use]
    pub fn recovery(&self) -> Recovery {
        match self {
            Self::Invalid(_) | Self::UnusableSettings(_) | Self::Decode { .. } => {
                Recovery::Escalate(Escalation::Ask(Fix::ChangeSettings))
            }
            Self::Adapter { recovery, .. } => *recovery,
            Self::Faulted(_) => Recovery::Escalate(Escalation::Restart),
            // A rejected request; a bug, which a new process repeats; the
            // answer while shutting down.
            Self::StaleRevision { .. }
            | Self::NeedsScreenSync
            | Self::Panicked(_)
            | Self::Closed => Recovery::Escalate(Escalation::FAIL_FAST),
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

    #[test]
    fn an_adapter_failure_fails_fast_unless_built_with_its_recovery() {
        let fail_fast = Recovery::Escalate(Escalation::FAIL_FAST);
        let io = || std::io::Error::other("unplugged");
        assert_eq!(Error::adapter("cannot read", io()).recovery(), fail_fast);
        assert_eq!(Error::other("cannot read").recovery(), fail_fast);

        let stall = Recovery::Retry(crate::Converges::StartStall);
        for (plain, recovering) in [
            (
                Error::adapter("cannot read", io()),
                Error::adapter_recovering("cannot read", io(), stall),
            ),
            (
                Error::other("cannot read"),
                Error::other_recovering("cannot read", stall),
            ),
        ] {
            assert_eq!(recovering.recovery(), stall);
            assert_eq!(recovering.to_string(), plain.to_string());
            assert_eq!(chain(&recovering), chain(&plain));
            assert_eq!(
                StdError::source(&recovering).map(ToString::to_string),
                StdError::source(&plain).map(ToString::to_string)
            );
            assert_eq!(recovering.section(), None);
            let placed = recovering.in_section(Section::Output);
            assert_eq!(placed.section(), Some(Section::Output));
            assert_eq!(placed.recovery(), stall);
        }
    }

    #[test]
    fn every_other_error_recovers_as_its_kind_says() {
        let settings = Recovery::Escalate(Escalation::Ask(Fix::ChangeSettings));
        let fail_fast = Recovery::Escalate(Escalation::FAIL_FAST);
        let invalid = Error::Invalid(ValidationError::new("capture.fps", "zero"));
        let decode = Error::Decode {
            kind: "fake".into(),
            field: "capture".into(),
            source: Arc::new(serde_json::from_str::<u8>("x").unwrap_err()),
        };
        let stale = Error::StaleRevision {
            expected: Revision::default(),
            current: Revision::default(),
        };
        for (error, recovery) in [
            (Error::UnusableSettings(Arc::new(invalid.clone())), settings),
            (invalid, settings),
            (decode, settings),
            (stale, fail_fast),
            (Error::NeedsScreenSync, fail_fast),
            (
                Error::Faulted(SessionPhase::ClosingSource),
                Recovery::Escalate(Escalation::Restart),
            ),
            (Error::Panicked("session"), fail_fast),
            (Error::Closed, fail_fast),
        ] {
            assert_eq!(error.recovery(), recovery, "{error}");
        }
    }

    #[test]
    fn only_a_fault_asks_for_a_new_process() {
        let restart = Recovery::Escalate(Escalation::Restart);
        assert_eq!(
            Error::Faulted(SessionPhase::IdlingOutput).recovery(),
            restart
        );
        assert_ne!(Error::Panicked("session").recovery(), restart, "a bug");
        assert_ne!(Error::Closed.recovery(), restart, "shutting down");
    }

    #[test]
    fn attempts_refuse_an_error_that_does_not_say_their_convergence() {
        let now = std::time::Instant::now();
        let mut attempts = crate::Attempts::new(crate::Converges::StartStall);
        let fail_fast = Err(Escalation::FAIL_FAST);
        assert_eq!(
            attempts.failed(Error::other("x").recovery(), now),
            fail_fast
        );
        let faulted = Error::Faulted(SessionPhase::ClosingSource).recovery();
        assert_eq!(attempts.failed(faulted, now), Err(Escalation::Restart));
        let stall = Recovery::Retry(crate::Converges::StartStall);
        let said = Error::other_recovering("stalled", stall).recovery();
        assert_eq!(attempts.failed(said, now), Ok(now));
    }
}
