//! The supervisor's transitions, one literal row each: from a state, a
//! happening at a time, the effects, the next state and the failure shown.
use super::*;
use crate::machine::phase::{Phase, Sent, Then};
use crate::machine::recovery::Restart;
use Restart::{Available, Spent};
use prysm_webos_link::Fix;

/// A phase without its times or problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Is {
    Idle,
    Running(Restart),
    Stopping(Sent, Next),
    Asking,
}

/// What follows a stopping worker, without its problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    Idle,
    Restart,
    Ask,
}

fn is(phase: &Phase) -> Is {
    match phase {
        Phase::Idle => Is::Idle,
        Phase::Running(run) => Is::Running(run.restart),
        Phase::Stopping(stopping) => Is::Stopping(
            stopping.sent,
            match stopping.then {
                Then::Idle => Next::Idle,
                Then::Restart => Next::Restart,
                Then::Ask(_) => Next::Ask,
            },
        ),
        Phase::Asking(_) => Is::Asking,
    }
}

/// A full machine (target off) in a state, built from a worker started at
/// 0 (by power, except `Sleeping`).
#[derive(Debug, Clone, Copy)]
enum Given {
    /// No worker: the TV sleeps.
    Sleeping,
    Running,
    /// Running after a restart, no frame yet.
    Restarted,
    /// The worker said why its run ends (`Event::Failed`).
    Said(Escalation),
    /// Stopped for standby at 500.
    StoppingAsked,
    /// Stopped at 500 for a reported fault, to restart.
    StoppingToRestart,
    /// Stopped at 500 for a broken link, to ask.
    StoppingToAsk,
    /// `StoppingAsked`, killed at 8 500.
    KilledAsked,
    /// `StoppingToRestart`, killed at 8 500.
    KilledToRestart,
    /// A broken link at 500, reaped at 600.
    Asking,
}

fn start() -> Vec<Effect> {
    vec![spawn_full(LightingTarget::Off), video(Video::Unknown)]
}

/// A report of `escalation`, for `message`.
fn faulty(escalation: Escalation, message: &str) -> Input {
    full_status(FullStatus {
        failure: Some(failure(escalation, message)),
        ..full_report(LightingTarget::Off)
    })
}

fn capturing() -> Input {
    full_status(FullStatus {
        capturing: true,
        ..full_report(LightingTarget::Off)
    })
}

impl Given {
    fn steps(self) -> Vec<(u64, Input, Vec<Effect>)> {
        let power = (0, Pw(Active), start());
        let asleep = (
            500,
            Pw(Asleep),
            vec![video(Video::Unavailable), Effect::Terminate],
        );
        let fault = (
            500,
            faulty(Escalation::Restart, "release unconfirmed"),
            vec![Effect::Terminate],
        );
        let broken = (
            500,
            Input::WorkerBroken("x".into()),
            vec![Effect::Terminate],
        );
        let kill = (8_500, Tick, vec![Effect::Kill]);
        match self {
            Self::Sleeping => vec![],
            Self::Running => vec![power],
            Self::Restarted => vec![power, (100, Exited(Exit::Signal(9)), start())],
            Self::Said(escalation) => vec![power, (500, failed_with(escalation, "said"), vec![])],
            Self::StoppingAsked => vec![power, asleep],
            Self::StoppingToRestart => vec![power, fault],
            Self::StoppingToAsk => vec![power, broken],
            Self::KilledAsked => vec![power, asleep, kill],
            Self::KilledToRestart => vec![power, fault, kill],
            Self::Asking => vec![power, broken, (600, Exited(Exit::Code(1)), vec![])],
        }
    }

    fn build(self, t0: Instant, unreadable: bool) -> Machine {
        let mut m = full();
        if unreadable {
            m = m.with_unreadable_preferences(UNREADABLE);
        }
        script(&mut m, t0, self.steps());
        m
    }
}

type Row = (
    Given,
    u64,
    Input,
    Vec<Effect>,
    Is,
    Option<(Asked, &'static str)>,
);

/// While a worker runs: what it reports, how it ends, and what the person
/// or the TV does.
#[rustfmt::skip]
fn running() -> Vec<Row> {
    use Given::{Restarted, Running, Said};
    let term = || vec![Effect::Terminate];
    let restarting = Is::Stopping(Sent::Terminate, Next::Restart);
    let giving_up = Is::Stopping(Sent::Terminate, Next::Ask);
    let again = Some((TRY_AGAIN, "again after a restart: release unconfirmed"));
    vec![
        (Running, 1_000, capturing(), vec![], Is::Running(Available), None),
        (Running, 1_000, faulty(Escalation::Restart, "release unconfirmed"), term(), restarting, None),
        (
            Running, 1_000, faulty(Escalation::Ask(Fix::ChangeSettings), "bad"),
            vec![], Is::Running(Available), Some((Asked::Ask(Fix::ChangeSettings), "bad")),
        ),
        (
            Running, 1_000, faulty(Escalation::Unanswered, "no answer"), vec![], Is::Running(Available),
            Some((Asked::Unanswered, "no answer")),
        ),
        (
            Running, 1_000, failed_with(Escalation::Ask(Fix::SetUp), "denied"), vec![],
            Is::Running(Available), Some((Asked::Ask(Fix::SetUp), "denied")),
        ),
        (Running, 1_000, failed_with(Escalation::Restart, "faulted"), vec![], Is::Running(Available), None),
        (Running, 1_000, Exited(Exit::Code(0)), start(), Is::Running(Spent), None),
        (Running, 1_000, Exited(Exit::Code(1)), start(), Is::Running(Spent), None),
        (Running, 1_000, Exited(Exit::Signal(9)), start(), Is::Running(Spent), None),
        (
            Running, 1_000, Input::WorkerBroken("y".into()), term(), giving_up,
            Some((TRY_AGAIN, "worker link broke: y")),
        ),
        (
            Running, 1_000, Input::SpawnFailed("s".into()), vec![], Is::Asking,
            Some((TRY_AGAIN, "cannot start the worker: s")),
        ),
        (Running, 14_999, Tick, vec![], Is::Running(Available), None),
        (Running, 15_000, Tick, term(), restarting, None),
        (Running, 1_000, Target(LightingTarget::Effects), vec![send_target(LightingTarget::Effects, 1)], Is::Running(Available), None),
        (
            Running, 1_000, Pw(Asleep), vec![video(Video::Unavailable), Effect::Terminate],
            Is::Stopping(Sent::Terminate, Next::Idle), None,
        ),
        (Running, 1_000, Input::Shutdown, term(), Is::Stopping(Sent::Terminate, Next::Idle), None),
        // Why it said its run ends wins over how it ends.
        (Said(Escalation::Ask(Fix::SetUp)), 1_000, Exited(Exit::Code(1)), vec![], Is::Asking, Some((Asked::Ask(Fix::SetUp), "said"))),
        (Said(Escalation::Ask(Fix::SetUp)), 15_500, Tick, term(), giving_up, Some((Asked::Ask(Fix::SetUp), "said"))),
        (Said(Escalation::Restart), 1_000, Exited(Exit::Code(1)), start(), Is::Running(Spent), None),
        // The restart is spent: the next fault before a frame asks.
        (Restarted, 1_000, faulty(Escalation::Restart, "release unconfirmed"), term(), giving_up, again),
        (
            Restarted, 1_000, Exited(Exit::Signal(9)), vec![], Is::Asking,
            Some((TRY_AGAIN, "again after a restart: worker killed by signal 9")),
        ),
        (
            Restarted, 15_100, Tick, term(), giving_up,
            Some((TRY_AGAIN, "again after a restart: worker sent nothing for 15 s")),
        ),
        (Restarted, 1_000, status(LightingTarget::Off), vec![], Is::Running(Spent), None),
        // A frame, or the person's choice, gives it back.
        (Restarted, 1_000, capturing(), vec![], Is::Running(Available), None),
        (Restarted, 1_000, Target(LightingTarget::Effects), vec![send_target(LightingTarget::Effects, 1)], Is::Running(Available), None),
    ]
}

/// While a worker stops, and while the person is asked.
#[rustfmt::skip]
fn stopping_and_asking() -> Vec<Row> {
    use Given::{Asking, KilledAsked, Sleeping, KilledToRestart, StoppingAsked, StoppingToAsk, StoppingToRestart};
    use Sent::{Kill, Terminate};
    let broken = Some((TRY_AGAIN, "worker link broke: x"));
    let abandoned = Some((Asked::Ask(Fix::RestartTv), "worker not reaped 8 s after its kill"));
    let effects = || Target(LightingTarget::Effects);
    let asked = Is::Stopping(Terminate, Next::Idle);
    vec![
        (Sleeping, 1_000, Pw(Active), start(), Is::Running(Available), None),
        (Sleeping, 1_000, effects(), vec![], Is::Idle, None),
        (Sleeping, 1_000, Tick, vec![], Is::Idle, None),
        // A stop asked for ends the run whatever it reports or how it ends;
        // the next worker is a new process anyway.
        (StoppingAsked, 1_000, Exited(Exit::Code(0)), vec![], Is::Idle, None),
        (StoppingAsked, 1_000, Exited(Exit::Signal(9)), vec![], Is::Idle, None),
        (StoppingAsked, 1_000, faulty(Escalation::Restart, "release unconfirmed"), vec![], asked, None),
        (StoppingAsked, 1_000, failed_with(Escalation::Ask(Fix::SetUp), "denied"), vec![], asked, None),
        (StoppingAsked, 1_000, Pw(Active), vec![], asked, None),
        (StoppingAsked, 8_499, Tick, vec![], asked, None),
        (StoppingAsked, 8_500, Tick, vec![Effect::Kill], Is::Stopping(Kill, Next::Idle), None),
        (KilledAsked, 9_000, Exited(Exit::Signal(9)), vec![], Is::Idle, None),
        (KilledAsked, 16_500, Tick, vec![Effect::Abandon], Is::Asking, abandoned),
        // A stop for a restart: decided once, it restarts unless the person acts.
        (StoppingToRestart, 1_000, Exited(Exit::Code(1)), start(), Is::Running(Spent), None),
        (StoppingToRestart, 1_000, failed_with(Escalation::Ask(Fix::SetUp), "denied"), vec![], Is::Stopping(Terminate, Next::Restart), None),
        (StoppingToRestart, 1_000, Input::WorkerBroken("y".into()), vec![], Is::Stopping(Terminate, Next::Restart), None),
        (StoppingToRestart, 8_500, Tick, vec![Effect::Kill], Is::Stopping(Kill, Next::Restart), None),
        (StoppingToRestart, 1_000, effects(), vec![], asked, None),
        (KilledToRestart, 9_000, Exited(Exit::Signal(9)), start(), Is::Running(Spent), None),
        (KilledToRestart, 16_500, Tick, vec![Effect::Abandon], Is::Asking, abandoned),
        // A stop to ask: the problem is shown while it stops.
        (StoppingToAsk, 1_000, Exited(Exit::Code(1)), vec![], Is::Asking, broken),
        (StoppingToAsk, 1_000, effects(), vec![], asked, None),
        (StoppingToAsk, 1_000, Pw(Asleep), vec![], asked, None),
        // Asked: only the person's or the TV's action ends it.
        (Asking, 1_000, effects(), vec![spawn_full(LightingTarget::Effects), video(Video::Unknown)], Is::Running(Available), None),
        (Asking, 1_000, Pw(Asleep), vec![], Is::Idle, None),
        (Asking, 1_000, Pw(ScreenSaver), vec![], Is::Asking, broken),
        (Asking, 1_000, Input::Preferences(Preferences::default()), vec![], Is::Asking, broken),
        (Asking, 1_000, Input::Shutdown, vec![], Is::Idle, None),
        (Asking, 60_000, Tick, vec![], Is::Asking, broken),
        (Asking, 1_000, capturing(), vec![], Is::Asking, broken),
        (Asking, 1_000, Exited(Exit::Code(1)), vec![], Is::Asking, broken),
        (Asking, 1_000, Enabled(true), vec![], Is::Asking, broken),
    ]
}

/// Every row; with unreadable preferences, each the same, and the
/// unreadable file reported beside it until preferences are saved.
#[test]
fn every_transition_is_one_row() {
    let t0 = Instant::now();
    for (from, at, input, effects, to, shown) in running().into_iter().chain(stopping_and_asking())
    {
        for unreadable in [false, true] {
            let mut m = from.build(t0, unreadable);
            let what = format!("{from:?} x {input:?} at {at} ms, unreadable {unreadable}");
            let saves = matches!(input, Input::Preferences(_));
            let done: Vec<_> = m
                .handle(input.clone(), t0 + Duration::from_millis(at))
                .into_iter()
                .filter(|effect| *effect != Effect::Publish)
                .collect();
            assert_eq!(done, effects, "{what}");
            assert_eq!(is(&m.phase), to, "{what}");
            let state = m.state();
            assert_eq!(asked(&state), shown, "{what}");
            let kept = (unreadable && !saves).then_some(UNREADABLE);
            assert_eq!(state.unreadable.as_deref(), kept, "{what}");
        }
    }
}
