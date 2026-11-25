//! Where the worker is, with what is valid only there.
use super::Package;
use super::recovery::{After, Restart, after};
use crate::state::Problem;
use prysm_webos_link::{Failure, FullStatus, SlimStatus, WorkerStatus};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Phase {
    Idle,
    Running(Run),
    Stopping(Stopping),
    /// No worker: the person is asked. Left only by their or the TV's
    /// action ([`super::Machine::dismiss`]).
    Asking(Problem),
}

/// A running worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Run {
    /// When it last sent a line.
    pub(super) heard: Instant,
    pub(super) report: Report,
    pub(super) restart: Restart,
    /// Why it says its run ends ([`prysm_webos_link::Event::Failed`]): its
    /// exit follows.
    pub(super) ended: Option<Failure>,
}

impl Run {
    /// What it asks of the person while it runs: why its run ends, else
    /// its report's failure (full; a slim worker's are its link's own).
    pub(super) fn problem(&self) -> Option<Problem> {
        let reported = || self.report.full()?.failure.as_ref();
        let failure = self.ended.as_ref().or_else(reported)?;
        match after(self.restart, failure.clone()) {
            After::Restart(_) => None,
            After::GiveUp(problem) | After::Ask(problem) => Some(problem),
        }
    }
}

/// A worker being stopped: it was sent `sent`; at `at` the next step is due
/// (a kill, then letting go).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Stopping {
    pub(super) at: Instant,
    pub(super) sent: Sent,
    pub(super) then: Then,
    /// Full: the portal address it last reported, until it was stopped and
    /// since (the portal answers until it exits).
    pub(super) portal: Option<String>,
}

/// The last signal a stopping worker was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Sent {
    Terminate,
    Kill,
}

/// What follows a stopping worker's exit. Decided once per worker: what it
/// reports while it stops changes nothing (a stop asked for ends its run,
/// and the next worker is a new process anyway). Only an action of the
/// person or the TV (`Idle`) and letting it go (asked) change it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Then {
    /// The person or the TV asked for the stop: nothing, or at once a new
    /// worker if one is wanted (new worker settings).
    Idle,
    /// A new process at once, its restart spent.
    Restart,
    /// The person is asked.
    Ask(Problem),
}

/// What the current worker last reported, in its package's terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Report {
    Full {
        /// The `sequence` of the last [`prysm_webos_link::Command::Target`]
        /// sent to this worker; 0 before any (it started with the package's
        /// target). A report that applied an older one does not change the
        /// target back.
        sent: u64,
        status: Option<FullStatus>,
    },
    Slim(Option<SlimStatus>),
}

impl Report {
    /// Nothing reported yet by a worker of `package`.
    pub(super) fn new(package: &Package) -> Self {
        match package {
            Package::Full { .. } => Self::Full {
                sent: 0,
                status: None,
            },
            Package::Slim { .. } => Self::Slim(None),
        }
    }

    /// Keep `status`; `false` (and nothing kept) when it is the other
    /// package's.
    pub(super) fn keep(&mut self, status: WorkerStatus) -> bool {
        match (self, status) {
            (Self::Full { status: kept, .. }, WorkerStatus::Full(status)) => *kept = Some(status),
            (Self::Slim(kept), WorkerStatus::Slim(status)) => *kept = Some(status),
            (Self::Full { .. }, WorkerStatus::Slim(_)) | (Self::Slim(_), WorkerStatus::Full(_)) => {
                return false;
            }
        }
        true
    }

    pub(super) fn full(&self) -> Option<&FullStatus> {
        match self {
            Self::Full { status, .. } => status.as_ref(),
            Self::Slim(_) => None,
        }
    }

    pub(super) fn slim(&self) -> Option<&SlimStatus> {
        match self {
            Self::Full { .. } => None,
            Self::Slim(status) => status.as_ref(),
        }
    }
}
