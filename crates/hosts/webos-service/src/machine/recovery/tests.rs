//! Recovery for each escalation and worker stage.
use super::*;

fn ask(asked: Asked, message: &str) -> Problem {
    Problem {
        asked,
        message: message.into(),
    }
}

#[test]
fn every_escalation_has_one_reading() {
    use WorkerStage::{Restarting, Stable, Starting};
    let restart = || After::Restart("x".into());
    let gave_up = || After::GiveUp(ask(Asked::Ask(Fix::TryAgain), "again after a restart: x"));
    let rows = [
        (Escalation::Restart, Starting, restart()),
        (Escalation::Restart, Stable, restart()),
        (Escalation::Restart, Restarting, gave_up()),
        (
            Escalation::Unanswered,
            Starting,
            After::Ask(ask(Asked::Unanswered, "x")),
        ),
        (
            Escalation::Unanswered,
            Restarting,
            After::Ask(ask(Asked::Unanswered, "x")),
        ),
        (
            Escalation::Unanswered,
            Stable,
            After::Ask(ask(Asked::Unanswered, "x")),
        ),
    ];
    let asks = Fix::ALL.into_iter().flat_map(|fix| {
        [Starting, Restarting, Stable].map(|stage| {
            (
                Escalation::Ask(fix),
                stage,
                After::Ask(ask(Asked::Ask(fix), "x")),
            )
        })
    });
    for (escalation, stage, follows) in rows.into_iter().chain(asks) {
        let failure = Failure {
            escalation,
            message: "x".into(),
        };
        assert_eq!(after(stage, failure), follows, "{escalation:?}, {stage:?}");
    }
}
