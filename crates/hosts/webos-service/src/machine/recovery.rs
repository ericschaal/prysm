//! What follows a failed worker: the supervisor's own causes, classified
//! once ([`Lost::escalation`]), and the one reading of an escalation
//! ([`after`]).
use prysm_webos_link::{Escalation, Failure, Fix, KILL_AFTER, SILENCE, WorkerFailure};

/// Why the supervisor lost a worker, by its own observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Lost {
    /// The program could not be started.
    Spawn(String),
    /// Its event stream broke (a bad line, or the other package's status).
    Broken(String),
    /// It sent nothing for [`SILENCE`].
    Silent,
    /// Still not reaped [`KILL_AFTER`] after its kill.
    Abandoned,
}

impl Lost {
    /// What is left of it: the supervisor's one classification of its own
    /// causes.
    pub(super) fn escalation(&self) -> Escalation {
        match self {
            // A missing or unrunnable program, or a worker that speaks
            // another protocol: a new process does the same.
            Self::Spawn(_) | Self::Broken(_) => Escalation::FAIL_FAST,
            // Hung: a new process has none of the old one's state.
            Self::Silent => Escalation::Restart,
            // It may hold the capture until the TV restarts.
            Self::Abandoned => Escalation::Ask(Fix::RestartTv),
        }
    }

    pub(super) fn failure(&self) -> Failure {
        let message = match self {
            Self::Spawn(why) => format!("cannot start the worker: {why}"),
            Self::Broken(why) => format!("worker link broke: {why}"),
            Self::Silent => format!("worker sent nothing for {} s", SILENCE.as_secs()),
            Self::Abandoned => format!(
                "worker not reaped {} s after its kill",
                KILL_AFTER.as_secs()
            ),
        };
        Failure {
            escalation: self.escalation(),
            message,
        }
    }
}

/// What the person is asked: an escalation nothing on the TV recovers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Asked {
    /// The lights do not answer.
    Unanswered,
    Ask(Fix),
}

/// A failure the person is asked about, and its message with the causes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Problem {
    pub(super) asked: Asked,
    pub(super) message: String,
}

/// What follows a worker that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum After {
    /// A new process, after the backoff; why the worker ended.
    Restart(String),
    /// No new process until the person acts.
    Ask(Problem),
}

/// The one place the supervisor reads an escalation: what follows a worker
/// that ends for `failure`.
pub(super) fn after(failure: Failure) -> After {
    let Failure {
        escalation,
        message,
    } = failure;
    let ask = |asked| {
        After::Ask(Problem {
            asked,
            message: message.clone(),
        })
    };
    match escalation {
        Escalation::Restart => After::Restart(message.clone()),
        Escalation::Unanswered => ask(Asked::Unanswered),
        Escalation::Ask(fix) => ask(Asked::Ask(fix)),
    }
}

/// Interim, replaced by the Luna job, which publishes the problem with what
/// is asked: `problem` as the TV app reads a failure today, kind `other`
/// (shown as one that "Try again" may fix), its message ending in what is
/// asked, for the app's diagnostics.
pub(super) fn interim_failure(problem: &Problem) -> WorkerFailure {
    let asked = match problem.asked {
        Asked::Unanswered => "check that the lights are on and on the network",
        Asked::Ask(Fix::TryAgain) => "try again",
        Asked::Ask(Fix::ChangeSettings) => "change the settings",
        Asked::Ask(Fix::SetUp) => "set up the service again",
        Asked::Ask(Fix::Unsupported) => "not supported on this TV",
        Asked::Ask(Fix::RestartTv) => "restart the TV",
    };
    WorkerFailure::other(format!("{} ({asked})", problem.message))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each of the supervisor's own causes, what is left of it and what
    /// follows.
    #[test]
    fn the_supervisors_causes_are_classified_once() {
        let ask = |fix, message: &str| {
            After::Ask(Problem {
                asked: Asked::Ask(fix),
                message: message.into(),
            })
        };
        let rows = [
            (
                Lost::Spawn("no such file".into()),
                ask(Fix::TryAgain, "cannot start the worker: no such file"),
            ),
            (
                Lost::Broken("bad line".into()),
                ask(Fix::TryAgain, "worker link broke: bad line"),
            ),
            (
                Lost::Silent,
                After::Restart("worker sent nothing for 15 s".into()),
            ),
            (
                Lost::Abandoned,
                ask(Fix::RestartTv, "worker not reaped 8 s after its kill"),
            ),
        ];
        for (lost, follows) in rows {
            assert_eq!(after(lost.failure()), follows, "{lost:?}");
        }
    }
}
