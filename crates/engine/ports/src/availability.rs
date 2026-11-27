//! The lifecycle contract shared by capture and output adapters.

use crate::{IdleMode, LedOutput};
use std::sync::Arc;

/// What the runtime can ask an adapter to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// The adapter lifecycle is stopped.
    Down,
    /// The adapter can be claimed. Opening may still fail.
    Ready,
    /// A claim owns initialization, use or teardown.
    Active,
    /// A known prerequisite can recover autonomously.
    Waiting,
    /// The current configuration has no autonomous recovery.
    Blocked,
}

/// Current readiness and its typed cause, independent of a held claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness<C> {
    Ready,
    Waiting(C),
    Blocked(C),
}

impl<C> Readiness<C> {
    /// Transform the reported cause while preserving what can recover.
    pub fn map<D>(self, map: impl FnOnce(C) -> D) -> Readiness<D> {
        match self {
            Self::Ready => Readiness::Ready,
            Self::Waiting(cause) => Readiness::Waiting(map(cause)),
            Self::Blocked(cause) => Readiness::Blocked(map(cause)),
        }
    }
}

/// Whether this request can replace another controller's explicit choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenIntent {
    Explicit,
    Resume,
}

/// A Screen sync attempt either acquires a streaming handle or defers to
/// a device-local override, with no output handle acquired.
pub enum OutputOpen {
    Streaming(Box<dyn LedOutput>),
    Handoff(OutputObservation),
}

/// A held output is released either with conditional idle or as a handoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputRelease {
    Ordinary(IdleMode),
    Handoff,
    /// Release just the local handle before a separate device command.
    Local,
}

/// The exclusive claim for opening and using one adapter.
#[must_use]
#[derive(Debug)]
pub struct Claim(Arc<()>);

/// The owning session's proof that this claim entered teardown.
#[must_use]
#[derive(Debug)]
pub struct Release(Arc<()>);

#[derive(Debug)]
enum Held {
    Claimed(Arc<()>),
    Releasing(Arc<()>),
}

/// Pure transitions for one adapter. Callers serialize events through their
/// existing owner and execute effects with the returned handles.
#[derive(Debug)]
pub struct AdapterLifecycle<C> {
    readiness: Option<Readiness<C>>,
    held: Option<Held>,
}

impl<C> Default for AdapterLifecycle<C> {
    fn default() -> Self {
        Self {
            readiness: None,
            held: None,
        }
    }
}

impl<C> AdapterLifecycle<C> {
    #[must_use]
    pub const fn availability(&self) -> Availability {
        match (&self.held, &self.readiness) {
            (Some(_), _) => Availability::Active,
            (None, Some(Readiness::Ready)) => Availability::Ready,
            (None, Some(Readiness::Waiting(_))) => Availability::Waiting,
            (None, Some(Readiness::Blocked(_))) => Availability::Blocked,
            (None, None) => Availability::Down,
        }
    }

    /// The latest current readiness, including observations made during close.
    #[must_use]
    pub const fn readiness(&self) -> Option<&Readiness<C>> {
        self.readiness.as_ref()
    }

    /// Enable a stopped lifecycle with the adapter's initial readiness.
    pub fn start(&mut self, readiness: Readiness<C>) {
        if self.held.is_none() && self.readiness.is_none() {
            self.readiness = Some(readiness);
        }
    }

    /// Stop observing. A held resource remains Active until confirmed release.
    pub fn stop(&mut self) {
        self.readiness = None;
    }

    /// Publish current readiness from a notification or manual recheck.
    /// Use this for current observations from the adapter's owner, including
    /// notifications during release. The caller retires obsolete effects.
    pub fn observe(&mut self, readiness: Readiness<C>) {
        if self.readiness.is_some() {
            self.readiness = Some(readiness);
        }
    }

    /// Claim once, before initialization starts.
    pub fn claim(&mut self) -> Option<Claim> {
        if self.availability() != Availability::Ready {
            return None;
        }
        let owner = Arc::new(());
        self.held = Some(Held::Claimed(Arc::clone(&owner)));
        Some(Claim(owner))
    }

    /// Enter teardown once. The owner keeps the returned handle during close.
    pub fn begin_release(&mut self, claim: &Claim) -> Option<Release> {
        let Some(Held::Claimed(owner)) = &self.held else {
            return None;
        };
        if !Arc::ptr_eq(owner, &claim.0) {
            return None;
        }
        self.held = Some(Held::Releasing(Arc::clone(&claim.0)));
        Some(Release(Arc::clone(&claim.0)))
    }

    /// Confirm local release. Current readiness now governs the next claim.
    /// An unsafe or unconfirmed release keeps the handle and remains Active.
    pub fn released(&mut self, release: &Release) -> Option<Availability> {
        let Some(Held::Releasing(owner)) = &self.held else {
            return None;
        };
        if !Arc::ptr_eq(owner, &release.0) {
            return None;
        }
        self.held = None;
        Some(self.availability())
    }
}

/// Physical power as reported by an output. An unknown value is represented by
/// the absence of an observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Power {
    Off,
    On,
}

/// Which display behavior the output reports independently of physical power.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputDisplay {
    Idle,
    Live,
    Override,
}

/// Output facts for startup reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputObservation {
    pub power: Power,
    pub display: OutputDisplay,
}

/// Whether an output can report device state, and what it reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputObservationState {
    Unsupported,
    Pending,
    Observed(OutputObservation),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    enum Cause {
        Unplugged,
    }

    #[test]
    fn release_protects_the_claim_and_publishes_reconnect_during_close() {
        let mut adapter = AdapterLifecycle::default();
        assert_eq!(adapter.availability(), Availability::Down);
        adapter.start(Readiness::Ready);
        let claim = adapter.claim().unwrap();
        assert_eq!(adapter.availability(), Availability::Active);
        assert!(adapter.claim().is_none());

        adapter.observe(Readiness::Waiting(Cause::Unplugged));
        let release = adapter.begin_release(&claim).unwrap();
        assert!(adapter.begin_release(&claim).is_none());
        assert!(adapter.claim().is_none());

        adapter.observe(Readiness::Ready);
        assert_eq!(adapter.availability(), Availability::Active);
        assert_eq!(adapter.released(&release), Some(Availability::Ready));

        let next = adapter.claim().unwrap();
        assert!(adapter.begin_release(&claim).is_none());
        assert_eq!(adapter.released(&release), None);
        let next_release = adapter.begin_release(&next).unwrap();
        assert_eq!(adapter.released(&release), None);
        assert_eq!(adapter.released(&next_release), Some(Availability::Ready));
    }

    #[test]
    fn concurrent_adapters_reject_each_others_claims_and_release_proofs() {
        let mut capture = AdapterLifecycle::<Cause>::default();
        let mut output = AdapterLifecycle::<Cause>::default();
        capture.start(Readiness::Ready);
        output.start(Readiness::Ready);
        let a = capture.claim().unwrap();
        let b = output.claim().unwrap();
        assert!(capture.begin_release(&b).is_none());
        assert!(output.begin_release(&a).is_none());
        let a = capture.begin_release(&a).unwrap();
        let b = output.begin_release(&b).unwrap();
        assert!(capture.released(&b).is_none());
        assert!(output.released(&a).is_none());
        assert_eq!(capture.availability(), Availability::Active);
        assert_eq!(output.availability(), Availability::Active);
        assert_eq!(capture.released(&a), Some(Availability::Ready));
        assert_eq!(output.released(&b), Some(Availability::Ready));
    }

    #[test]
    fn stop_keeps_unconfirmed_release_exclusive() {
        let mut adapter = AdapterLifecycle::<Cause>::default();
        adapter.start(Readiness::Ready);
        let claim = adapter.claim().unwrap();
        let release = adapter.begin_release(&claim).unwrap();
        adapter.stop();
        adapter.start(Readiness::Ready);
        assert_eq!(adapter.availability(), Availability::Active);
        assert!(adapter.claim().is_none());
        assert_eq!(adapter.released(&release), Some(Availability::Down));
        adapter.start(Readiness::Ready);
        assert!(adapter.claim().is_some());
    }
}
