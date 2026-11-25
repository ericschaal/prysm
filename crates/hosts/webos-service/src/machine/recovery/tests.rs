//! The classification and the one reading, as literal rows.
use super::*;

fn ask(asked: Asked, message: &str) -> Problem {
    Problem {
        asked,
        message: message.into(),
    }
}

/// Each of the supervisor's own causes: what is left of it, and its
/// message.
#[test]
fn the_supervisors_causes_are_classified_once() {
    let rows = [
        (
            Lost::Spawn("no such file".into()),
            Escalation::FAIL_FAST,
            "cannot start the worker: no such file",
        ),
        (
            Lost::Broken("bad line".into()),
            Escalation::FAIL_FAST,
            "worker link broke: bad line",
        ),
        (
            Lost::Silent,
            Escalation::Restart,
            "worker sent nothing for 15 s",
        ),
        (
            Lost::Exited(Exit::Code(0)),
            Escalation::Restart,
            "worker exited with code 0",
        ),
        (
            Lost::Exited(Exit::Code(1)),
            Escalation::Restart,
            "worker exited with code 1",
        ),
        (
            Lost::Exited(Exit::Signal(9)),
            Escalation::Restart,
            "worker killed by signal 9",
        ),
        (
            Lost::Abandoned,
            Escalation::Ask(Fix::RestartTv),
            "worker not reaped 8 s after its kill",
        ),
    ];
    for (lost, escalation, message) in rows {
        let failure = lost.failure();
        assert_eq!(failure.escalation, escalation, "{lost:?}");
        assert_eq!(failure.message, message, "{lost:?}");
    }
}

/// Every escalation with each restart: what follows.
#[test]
fn every_escalation_has_one_reading() {
    use Restart::{Available, Spent};
    let restart = || After::Restart("x".into());
    let gave_up = || After::GiveUp(ask(Asked::Ask(Fix::TryAgain), "again after a restart: x"));
    let rows = [
        (Escalation::Restart, Available, restart()),
        (Escalation::Restart, Spent, gave_up()),
        (
            Escalation::Unanswered,
            Available,
            After::Ask(ask(Asked::Unanswered, "x")),
        ),
        (
            Escalation::Unanswered,
            Spent,
            After::Ask(ask(Asked::Unanswered, "x")),
        ),
    ];
    let asks = Fix::ALL.into_iter().flat_map(|fix| {
        [Available, Spent].map(|restart| {
            (
                Escalation::Ask(fix),
                restart,
                After::Ask(ask(Asked::Ask(fix), "x")),
            )
        })
    });
    for (escalation, restart, follows) in rows.into_iter().chain(asks) {
        let failure = Failure {
            escalation,
            message: "x".into(),
        };
        assert_eq!(
            after(restart, failure),
            follows,
            "{escalation:?}, {restart:?}"
        );
    }
}
