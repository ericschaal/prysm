use super::end::hidden;
use super::publish::unpublished;
use super::*;
use prysm_ports::Section;

fn failure() -> Error {
    Error::other("cannot idle").in_section(Section::Output)
}

/// Every failure after a session end's first is found, in order; the
/// first is the result, and a fault is in the state.
#[test]
fn failures_behind_a_session_ends_first_are_found() {
    let end = |exit: Exit, released: Result<()>, left: Result<IdleMode>| SessionEnd {
        exit,
        released,
        left,
    };
    let release = || Err(Error::other("cannot close"));
    let idle = || Err(failure());
    let failed = || Exit::Failed(Error::other("source failed"));
    let found = |end: &SessionEnd| hidden(end).map(ToString::to_string).collect::<Vec<_>>();
    let none: [&str; 0] = [];
    assert_eq!(
        found(&end(failed(), release(), idle())),
        ["cannot close", "cannot idle"]
    );
    assert_eq!(found(&end(failed(), Ok(()), idle())), ["cannot idle"]);
    assert_eq!(found(&end(failed(), Ok(()), Ok(IdleMode::Off))), none);
    assert_eq!(
        found(&end(Exit::Stopped, release(), idle())),
        ["cannot idle"]
    );
    for exit in [Exit::Stopped, Exit::Ended] {
        assert_eq!(
            found(&end(exit, release(), Ok(IdleMode::Off))),
            none,
            "it is the result"
        );
    }
    assert_eq!(
        found(&end(
            failed(),
            Ok(()),
            Err(Error::Faulted(SessionPhase::IdlingOutput))
        )),
        none,
        "a fault is in the state"
    );
}

/// What a fault leaves unpublished is logged; a failure a stopped or
/// effects state carries, a fault's own error and success are not.
#[test]
fn a_fault_logs_the_error_it_ended_in() {
    use SessionPhase::{ClosingSource, SourceUnreleased};
    let release = || Err(Error::other("cannot close").in_section(Section::Capture));
    let cases: [(Lighting, Result<()>, Option<SessionPhase>); 6] = [
        (Lighting::UNRELEASED, release(), Some(SourceUnreleased)),
        (
            Lighting::Faulted(ClosingSource),
            Err(Error::Panicked("session thread")),
            Some(ClosingSource),
        ),
        (
            Lighting::UNRELEASED,
            Err(Error::Faulted(SourceUnreleased)),
            None,
        ),
        (Lighting::UNRELEASED, Ok(()), None),
        (Lighting::Stopped, release(), None),
        (Lighting::Effects, release(), None),
    ];
    for (lighting, result, logged) in cases {
        let found = unpublished(&lighting, &result).map(|(phase, _)| phase);
        assert_eq!(found, logged, "{result:?}");
    }
}

/// Every way a session can end, against what it leaves, and against
/// what a stop for the lights' own effects leaves and replies: never the
/// session's own failure.
#[test]
fn every_end_leaves_one_state_and_effects_replace_its_failure() {
    use SessionPhase::{IdlingOutput, SourceUnreleased};
    let stopped = || SessionState::default();
    let effects = || SessionState::Effects { failure: None };
    let idling = Error::Faulted(IdlingOutput);
    // (exit failed, released, left, state left, effects reply is ok)
    let cases: [(bool, bool, Result<IdleMode>, SessionState, bool); 10] = [
        (false, true, Ok(IdleMode::Off), stopped(), true),
        (false, true, Ok(IdleMode::Native), effects(), true),
        (false, true, Err(failure()), stopped(), false),
        (
            false,
            true,
            Err(idling.clone()),
            SessionState::Faulted(IdlingOutput),
            false,
        ),
        (
            false,
            false,
            Ok(IdleMode::Native),
            SessionState::Faulted(SourceUnreleased),
            false,
        ),
        (true, true, Ok(IdleMode::Off), stopped(), true),
        (true, true, Ok(IdleMode::Native), effects(), true),
        (true, true, Err(failure()), stopped(), false),
        (
            true,
            true,
            Err(idling),
            SessionState::Faulted(IdlingOutput),
            false,
        ),
        (
            true,
            false,
            Ok(IdleMode::Off),
            SessionState::Faulted(SourceUnreleased),
            false,
        ),
    ];
    for (failed, released, left, expected, reply_ok) in cases {
        let end = || SessionEnd {
            exit: if failed {
                Exit::Failed(Error::other("source failed"))
            } else {
                Exit::Stopped
            },
            released: if released {
                Ok(())
            } else {
                Err(Error::other("cannot close"))
            },
            left: left.clone(),
        };
        let row = format!("failed {failed}, released {released}, left {left:?}");
        let state = Lighting::left_by(&end()).state(None);
        assert_eq!(state.to_string(), expected.to_string(), "{row}");
        let (lighting, reply) = replaced_by_effects(end());
        assert_eq!(
            lighting.state(None).to_string(),
            expected.to_string(),
            "{row}"
        );
        assert_eq!(reply.is_ok(), reply_ok, "{row}: {reply:?}");
        if let Err(error) = reply {
            assert_ne!(error.to_string(), "source failed", "{row}");
        }
    }
}

/// A stopped or effects state carries the failure it is given; a fault
/// carries none.
#[test]
fn only_stopped_and_effects_carry_a_failure() {
    for (lighting, carries) in [
        (Lighting::Stopped, true),
        (Lighting::Effects, true),
        (Lighting::UNRELEASED, false),
    ] {
        let state = lighting.state(Some(failure()));
        assert_eq!(state.failure().is_some(), carries, "{state}");
    }
}
