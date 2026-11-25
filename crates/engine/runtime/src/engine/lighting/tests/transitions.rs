//! One literal table per transition: what each happening leaves.
use super::*;
use crate::leds::UNANSWERED_INTERVAL;
use prysm_ports::Escalation;
use std::time::Duration;

/// A wait's mode, first request and next try.
type Wait = (IdleMode, Instant, Instant);

fn wait(lighting: &Lighting) -> Option<Wait> {
    match *lighting {
        Lighting::Unanswered { mode, since, again } => Some((mode, since, again)),
        _ => None,
    }
}

fn unanswered() -> Error {
    Error::other_escalating("does not answer", Escalation::Unanswered)
}

/// A session end whose release worked or not.
fn end(exit: Exit, released: bool, left: Result<IdleMode>) -> SessionEnd {
    let released = if released {
        Ok(())
    } else {
        Err(Error::other("cannot close"))
    };
    SessionEnd {
        exit,
        released,
        left,
    }
}

/// What an end leaves, and the target: only an ended source under a
/// `ScreenSync` target turns it `Off`, whatever the lights were left in.
#[test]
fn an_ended_source_completes_screen_sync() {
    use IdleMode::{Native, Off};
    use Kind::{Effects, Stopped};
    use LightingTarget as Target;
    use SessionPhase::{ClosingSource, SourceUnreleased};
    let ended = |released, left| Ok(end(Exit::Ended, released, left));
    let stopped = || Ok(end(Exit::Stopped, true, Ok(Off)));
    let failed = || Ok(end(Exit::Failed(failure()), true, Ok(Off)));
    let (unreleased, closing) = (
        Kind::Faulted(SourceUnreleased),
        Kind::Faulted(ClosingSource),
    );
    let off = Some(Target::Off);
    // (end, target) against (lighting left, target left)
    #[rustfmt::skip]
    let cases = [
        (ended(true, Ok(Off)),               Target::ScreenSync, (Stopped, off)),
        (ended(true, Ok(Native)),            Target::ScreenSync, (Effects, off)),
        (ended(false, Ok(Off)),              Target::ScreenSync, (unreleased, off)),
        (ended(true, Ok(Off)),               Target::Effects,    (Stopped, None)),
        (ended(true, Ok(Off)),               Target::Off,        (Stopped, None)),
        (stopped(),                          Target::ScreenSync, (Stopped, None)),
        (failed(),                           Target::ScreenSync, (Stopped, None)),
        (Err(Error::Faulted(ClosingSource)), Target::ScreenSync, (closing, None)),
        (Err(Error::Panicked("thread")),     Target::ScreenSync, (unreleased, None)),
    ];
    for (row, (end, target, expected)) in cases.into_iter().enumerate() {
        let (lighting, target) = Lighting::ended(&end, target);
        assert_eq!((lighting.kind(), target), expected, "row {row}");
    }
}

/// What a stop reported, against what it leaves: a session that ended on
/// its own before a stop for the lights' own effects is replaced by them;
/// before a stop for `Off`, it is reported as it ended.
#[test]
fn a_stop_leaves_what_it_reported() {
    use IdleMode::{Native, Off};
    use SessionPhase::{IdlingOutput, SourceUnreleased};
    let asked = |released, left| Ok(end(Exit::Stopped, released, left));
    let own = || Ok(end(Exit::Ended, true, Ok(Off)));
    let fault = |phase| Some(Kind::Faulted(phase));
    // (mode, stop) against (outcome, lighting left, reply is ok)
    #[rustfmt::skip]
    let cases = [
        (Off,    Err(Error::Faulted(IdlingOutput)), ("failed", fault(IdlingOutput), Some(false))),
        (Native, Err(Error::Panicked("thread")),    ("failed", fault(SourceUnreleased), Some(false))),
        (Off,    asked(true, Ok(Off)),              ("idled", Some(Kind::Stopped), Some(true))),
        (Native, asked(true, Ok(Native)),           ("idled", Some(Kind::Effects), Some(true))),
        (Off,    asked(true, Err(unanswered())),    ("idled", Some(Kind::Stopped), Some(false))),
        (Off,    asked(false, Ok(Off)),             ("idled", fault(SourceUnreleased), Some(false))),
        (Native, own(),                             ("replaced by effects", None, None)),
        (Off,    own(),                             ("ended", None, None)),
    ];
    for (row, (mode, stop, expected)) in cases.into_iter().enumerate() {
        let found = match Lighting::stopped(mode, stop) {
            Stopped::Failed { lighting, .. } => ("failed", Some(lighting.kind()), Some(false)),
            Stopped::Idled { lighting, reply } => {
                ("idled", Some(lighting.kind()), Some(reply.is_ok()))
            }
            Stopped::ReplacedByEffects(_) => ("replaced by effects", None, None),
            Stopped::Ended(_) => ("ended", None, None),
        };
        assert_eq!(found, expected, "row {row}");
    }
}

/// Every target and lights against the step towards it, by (restart,
/// output changed): (no, no), (no, yes), (yes, no), (yes, yes).
#[test]
fn every_step_towards_a_target() {
    use IdleMode::{Native, Off};
    use Kind::{Effects, ScreenSync, Stopped, Unanswered};
    use LightingTarget as Target;
    use Step::{EndWait, Idle, MayNotBeOff, Restart, Start, Stay, Stop};
    let faulted = Kind::Faulted(SessionPhase::SourceUnreleased);
    let fault = Step::Faulted(SessionPhase::SourceUnreleased);
    let (start, after_wait) = (Start { ends_wait: false }, Start { ends_wait: true });
    #[rustfmt::skip]
    let cases = [
        (Target::ScreenSync, Stopped,    [start; 4]),
        (Target::ScreenSync, Effects,    [start; 4]),
        (Target::ScreenSync, ScreenSync, [Stay, Stay, Restart, Restart]),
        (Target::ScreenSync, Unanswered, [after_wait; 4]),
        (Target::ScreenSync, faulted,    [fault; 4]),
        (Target::Effects,    Stopped,    [Idle(Native); 4]),
        (Target::Effects,    Effects,    [Stay, Idle(Native), Stay, Idle(Native)]),
        (Target::Effects,    ScreenSync, [Stop(Native); 4]),
        (Target::Effects,    Unanswered, [Idle(Native); 4]),
        (Target::Effects,    faulted,    [fault; 4]),
        (Target::Off,        Stopped,    [MayNotBeOff, Stay, MayNotBeOff, Stay]),
        (Target::Off,        Effects,    [Idle(Off); 4]),
        (Target::Off,        ScreenSync, [Stop(Off); 4]),
        (Target::Off,        Unanswered, [Idle(Off), EndWait, Idle(Off), EndWait]),
        (Target::Off,        faulted,    [fault; 4]),
    ];
    let flags = [(false, false), (false, true), (true, false), (true, true)];
    for (target, kind, steps) in cases {
        for ((restart, output_changed), step) in flags.into_iter().zip(steps) {
            let found = kind.towards(target, restart, output_changed);
            let row = format!("{target:?} from {kind:?}, {restart} {output_changed}");
            assert_eq!(found, step, "{row}");
        }
    }
}

/// A restart starts its new session only from `Stopped` under `ScreenSync`.
#[test]
fn a_restart_starts_only_from_stopped_screen_sync() {
    use LightingTarget as Target;
    #[rustfmt::skip]
    let cases = [
        (Lighting::Stopped,    Target::ScreenSync, true),
        (Lighting::Stopped,    Target::Off,        false),
        (Lighting::Stopped,    Target::Effects,    false),
        (Lighting::Effects,    Target::ScreenSync, false),
        (Lighting::UNRELEASED, Target::ScreenSync, false),
    ];
    for (row, (lighting, target, restarts)) in cases.into_iter().enumerate() {
        assert_eq!(lighting.restarts(target), restarts, "row {row}");
    }
}

/// A session that could not start leaves the lights as they were, unless
/// its source's release is unconfirmed.
#[test]
fn an_unstarted_session_faults_only_when_unreleased() {
    let unreleased = Some(Kind::Faulted(SessionPhase::SourceUnreleased));
    let cases = [
        (Ok(()), None),
        (Err(Error::other("cannot close")), unreleased),
    ];
    for (released, kind) in cases {
        let found = Lighting::not_started(&released).map(|lighting| lighting.kind());
        assert_eq!(found, kind, "{released:?}");
    }
}

/// Only an output failure without a session says the lights may not be off.
#[test]
fn only_an_output_failure_when_stopped_may_leave_the_lights_on() {
    let (output, capture) = (failure(), Error::other("no").in_section(Section::Capture));
    let now = Instant::now();
    let waiting = Lighting::Unanswered {
        mode: IdleMode::Off,
        since: now,
        again: now,
    };
    #[rustfmt::skip]
    let cases = [
        (Lighting::Stopped, Some(&output),  true),
        (Lighting::Stopped, Some(&capture), false),
        (Lighting::Stopped, None,           false),
        (Lighting::Effects, Some(&output),  false),
        (waiting,           Some(&output),  false),
    ];
    for (row, (lighting, failure, may)) in cases.into_iter().enumerate() {
        assert_eq!(lighting.may_not_be_off(failure).is_some(), may, "row {row}");
    }
}

/// A session starts from the device as these lights leave it: off only
/// when nothing says they may not be.
#[test]
fn a_session_starts_from_what_the_lights_show() {
    use crate::leds::Device::{Left, Unknown};
    let now = Instant::now();
    let waiting = Lighting::Unanswered {
        mode: IdleMode::Off,
        since: now,
        again: now,
    };
    let (output, capture) = (failure(), Error::other("no").in_section(Section::Capture));
    #[rustfmt::skip]
    let cases = [
        (Lighting::Stopped, None,            Left(IdleMode::Off)),
        (Lighting::Stopped, Some(&capture),  Left(IdleMode::Off)),
        (Lighting::Stopped, Some(&output),   Unknown),
        (Lighting::Effects, None,            Left(IdleMode::Native)),
        (waiting,           None,            Unknown),
    ];
    for (row, (lighting, failure, shows)) in cases.into_iter().enumerate() {
        assert_eq!(lighting.shows(failure), shows, "row {row}");
    }
}

/// The rule for lights that do not answer: waited for only when every
/// condition holds. Each row after the first two breaks one.
#[test]
fn unanswered_lights_are_waited_for_only_by_the_rule() {
    use IdleMode::{Native, Off};
    use LightingTarget as Target;
    let (stopped, effects, unreleased) =
        (Lighting::Stopped, Lighting::Effects, Lighting::UNRELEASED);
    // (error, mode, left, target, shutting down) against waited for
    #[rustfmt::skip]
    let cases = [
        (unanswered(), Off,    &stopped,    Target::Off,        false, true),
        (unanswered(), Native, &stopped,    Target::Effects,    false, true),
        (failure(),    Off,    &stopped,    Target::Off,        false, false),
        (unanswered(), Native, &stopped,    Target::Off,        false, false),
        (unanswered(), Off,    &stopped,    Target::ScreenSync, false, false),
        (unanswered(), Off,    &effects,    Target::Off,        false, false),
        (unanswered(), Off,    &unreleased, Target::Off,        false, false),
        (unanswered(), Off,    &stopped,    Target::Off,        true,  false),
    ];
    for (row, (error, mode, left, target, shutting_down, waits)) in cases.into_iter().enumerate() {
        let found = Lighting::waits_for(&error, mode, left, target, shutting_down);
        assert_eq!(found, waits, "row {row}");
    }
}

/// What a request putting the lights in a mode leaves, from the lights
/// before it: a wait begins, goes on (its first request kept), or ends,
/// saying how long the lights took to answer.
#[test]
fn a_request_the_lights_do_not_answer_begins_or_continues_a_wait() {
    use IdleMode::{Native, Off};
    use LightingTarget as Target;
    let first = Instant::now();
    let now = first + Duration::from_secs(10);
    let again = now + UNANSWERED_INTERVAL;
    let waiting = |mode| Lighting::Unanswered {
        mode,
        since: first,
        again: first,
    };
    let (stopped, effects) = (|| Lighting::Stopped, || Lighting::Effects);
    let begins = |mode| ("begins", Some((mode, now, again)), true, None);
    let goes_on = ("goes on", Some((Off, first, again)), true, None);
    let left = |ok, answered| ("left", None, ok, answered);
    let ten = Some(Duration::from_secs(10));
    // (before, mode, left, reply, target, shutting down) against (outcome,
    // wait, reply is ok, answered after)
    #[rustfmt::skip]
    let cases = [
        (stopped(),     Off,    stopped(), Err(unanswered()), Target::Off,     false, begins(Off)),
        (waiting(Off),  Off,    stopped(), Err(unanswered()), Target::Off,     false, goes_on),
        (waiting(Off),  Native, stopped(), Err(unanswered()), Target::Effects, false, begins(Native)),
        (waiting(Off),  Off,    stopped(), Ok(()),            Target::Off,     false, left(true, ten)),
        (waiting(Native), Native, effects(), Ok(()),          Target::Effects, false, left(true, ten)),
        (stopped(),     Off,    stopped(), Err(unanswered()), Target::Off,     true,  left(false, None)),
        (waiting(Off),  Off,    stopped(), Err(failure()),    Target::Off,     false, left(false, None)),
        (stopped(),     Off,    stopped(), Ok(()),            Target::Off,     false, left(true, None)),
    ];
    for (row, (before, mode, after, reply, target, shutting_down, expected)) in
        cases.into_iter().enumerate()
    {
        let found = match before.after_idle(mode, after, reply, target, shutting_down, now) {
            Idled::Left {
                lighting,
                reply,
                answered,
            } => ("left", wait(&lighting), reply.is_ok(), answered),
            Idled::WaitBegins { lighting, .. } => ("begins", wait(&lighting), true, None),
            Idled::WaitGoesOn { lighting, .. } => ("goes on", wait(&lighting), true, None),
        };
        assert_eq!(found, expected, "row {row}");
    }
}

/// `Off` asked again switches the lights off again only while a published
/// failure says they may not be off; anything else steps towards the target.
#[test]
fn off_asked_again_switches_off_only_lights_that_may_be_on() {
    use LightingTarget as Target;
    let (output, capture) = (failure(), Error::other("no").in_section(Section::Capture));
    #[rustfmt::skip]
    let cases = [
        (Lighting::Stopped, Target::Off,        Some(&output),  true),
        (Lighting::Stopped, Target::Off,        Some(&capture), false),
        (Lighting::Stopped, Target::Off,        None,           false),
        (Lighting::Stopped, Target::Effects,    Some(&output),  false),
        (Lighting::Stopped, Target::ScreenSync, Some(&output),  false),
        (Lighting::Effects, Target::Off,        Some(&output),  false),
    ];
    for (row, (lighting, target, failure, again)) in cases.into_iter().enumerate() {
        assert_eq!(
            lighting.switches_off_again(target, failure),
            again,
            "row {row}"
        );
    }
}
