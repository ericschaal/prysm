//! What follows a failed worker: the supervisor's own causes, classified
//! once ([`Lost::escalation`]), and the one reading of an escalation
//! ([`after`]), with the one automatic recovery, a restart ([`Restart`]).
use super::Exit;
use crate::state::{Asked, Problem};
use prysm_webos_link::{Escalation, Failure, Fix, KILL_AFTER, SILENCE};

/// Why the supervisor lost a worker, by its own observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Lost {
    /// The program could not be started.
    Spawn(String),
    /// Its event stream broke (a bad line, or the other package's status).
    Broken(String),
    /// It sent nothing for [`SILENCE`].
    Silent,
    /// It exited without saying why ([`prysm_webos_link::Event::Failed`]),
    /// whatever the code or signal.
    Exited(Exit),
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
            // Hung, or ended without saying why (a crash, a signal, out of
            // memory): a new process has none of the old one's state.
            Self::Silent | Self::Exited(_) => Escalation::Restart,
            // It may hold the capture until the TV restarts.
            Self::Abandoned => Escalation::Ask(Fix::RestartTv),
        }
    }

    pub(super) fn failure(&self) -> Failure {
        let message = match self {
            Self::Spawn(why) => format!("cannot start the worker: {why}"),
            Self::Broken(why) => format!("worker link broke: {why}"),
            Self::Silent => format!("worker sent nothing for {} s", SILENCE.as_secs()),
            Self::Exited(Exit::Code(code)) => format!("worker exited with code {code}"),
            Self::Exited(Exit::Signal(signal)) => format!("worker killed by signal {signal}"),
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

/// Whether a worker that only a new process recovers gets one: the one
/// automatic recovery on the TV.
///
/// A new process clears what only a process holds (a driver, a hung call,
/// a lock). If the same happens again before the new one worked
/// ([`super::phase::Report::worked`]), with no action by the person or the
/// TV in between, a restart did not fix it, so the person is asked. A
/// working state, or such an action, gives the restart back: every restart
/// then follows a working state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Restart {
    Available,
    /// This worker is the restart.
    Spent,
}

/// What follows a worker's failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum After {
    /// The worker ends, and a new process starts at once; why.
    Restart(String),
    /// Only a new process recovers, and the restart is spent: the worker
    /// ends, and the person is asked.
    GiveUp(Problem),
    /// The person is asked. The worker need not end for it: a full worker
    /// keeps running, since its portal is where settings change.
    Ask(Problem),
}

/// The one place the supervisor reads an escalation: what follows the
/// worker's `failure`, with its `restart`.
pub(super) fn after(restart: Restart, failure: Failure) -> After {
    let Failure {
        escalation,
        message,
    } = failure;
    match escalation {
        Escalation::Restart => match restart {
            Restart::Available => After::Restart(message),
            Restart::Spent => After::GiveUp(Problem {
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

#[cfg(test)]
mod tests;
