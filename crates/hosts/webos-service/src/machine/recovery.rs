//! Worker recovery decisions from supervisor observations and reported failures.
use super::phase::Run;
use crate::state::{Asked, Problem};
use prysm_webos_link::{Escalation, Failure, Fix};

pub(super) fn fail_fast(message: String) -> Failure {
    Failure {
        escalation: Escalation::FAIL_FAST,
        message,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WorkerStage {
    Starting,
    Restarting,
    Stable,
}

/// What follows a worker's failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum After {
    /// The worker ends, and a new process starts at once; why.
    Restart(String),
    /// Stop the worker and ask the user.
    GiveUp(Problem),
    /// The person is asked. The worker need not end for it: a full worker
    /// keeps running, since its portal is where settings change.
    Ask(Problem),
}

/// Recovery requested by a worker, given its current stage.
pub(super) fn after(stage: WorkerStage, failure: Failure) -> After {
    let Failure {
        escalation,
        message,
    } = failure;
    match escalation {
        Escalation::Restart => match stage {
            WorkerStage::Starting | WorkerStage::Stable => After::Restart(message),
            WorkerStage::Restarting => After::GiveUp(Problem {
                asked: Asked::Ask(Fix::TryAgain),
                message: format!("again after a restart: {message}"),
            }),
        },
        Escalation::Unanswered => After::Ask(Problem {
            asked: Asked::Unanswered,
            message,
        }),
        Escalation::Ask(fix) => After::Ask(Problem {
            asked: Asked::Ask(fix),
            message,
        }),
    }
}

impl Run {
    pub(super) fn after_disconnect(&self, message: String) -> After {
        if let Some(failure) = self.ended.as_ref().or_else(|| self.report.failure()) {
            return after(self.stage, failure.clone());
        }
        match self.stage {
            WorkerStage::Stable => After::Restart(message),
            WorkerStage::Starting | WorkerStage::Restarting => After::GiveUp(Problem {
                asked: Asked::Ask(Fix::TryAgain),
                message,
            }),
        }
    }
}

#[cfg(test)]
mod tests;
