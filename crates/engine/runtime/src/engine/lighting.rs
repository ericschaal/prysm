//! What the lights are doing ([`Lighting`]), and what each happening leaves
//! them in. Its children are how the engine drives them (`target`, `end`,
//! `unanswered`) and publishes them (`publish`), each beside the helpers
//! only it needs: what a session end does not say, what a fault leaves
//! unpublished.
mod end;
mod publish;
mod target;
mod unanswered;

#[cfg(test)]
mod tests;

use crate::leds::{UNANSWERED_INTERVAL, UNREACHABLE};
use crate::meter::Meter;
use crate::session::{Session, SessionEnd};
use crate::worker::Exit;
use prysm_ports::{
    DeviceReport, Error, Escalation, IdleMode, LightingTarget, Result, Section, SessionPhase,
    SessionState,
};
use std::time::Duration;
use tokio::time::Instant;

/// What the lights are doing. The one truth for whether a session runs:
/// [`Core::set_lighting`](super::Core::set_lighting) publishes it.
#[expect(
    clippy::large_enum_variant,
    reason = "the engine holds exactly one, never moved per frame"
)]
pub(super) enum Lighting {
    /// No session; the output (if any was touched) is idle.
    Stopped,
    /// The output was put in its own effects, without a session.
    Effects,
    ScreenSync(Running),
    /// No session: the lights did not answer the request putting them in
    /// `mode`, the target's, which is tried again at `again`
    /// ([`Lighting::after_idle`]). Their mode is not known meanwhile.
    Unanswered {
        mode: IdleMode,
        /// The request they did not answer first.
        since: Instant,
        again: Instant,
    },
    /// Only shutdown is accepted, and only a process restart recovers.
    Faulted(SessionPhase),
}

impl Lighting {
    /// The published state, carrying `failure` where it can: screen sync
    /// runs on valid settings and has reached its target, and a fault says
    /// only its phase, so neither carries one. A wait for the lights is
    /// `Stopped`: no session, and the target not reached.
    fn state(&self, failure: Option<Error>) -> SessionState {
        match self {
            Self::Stopped | Self::Unanswered { .. } => SessionState::Stopped { failure },
            Self::Effects => SessionState::Effects { failure },
            Self::ScreenSync(_) => SessionState::ScreenSync,
            Self::Faulted(phase) => SessionState::Faulted(*phase),
        }
    }

    /// The published device report without a session: empty, but that the
    /// lights do not answer while they are waited for.
    fn device(&self) -> DeviceReport {
        match self {
            Self::Unanswered { .. } => UNREACHABLE,
            Self::Stopped | Self::Effects | Self::ScreenSync(_) | Self::Faulted(_) => {
                DeviceReport::default()
            }
        }
    }

    /// What a request putting the lights in `mode` leaves when it worked
    /// (their own effects for `Native`), and when it did not.
    pub(super) fn idled(mode: IdleMode, result: &Result<()>) -> Self {
        match (result, mode) {
            (Ok(()), IdleMode::Native) => Self::Effects,
            _ => Self::Stopped,
        }
    }

    /// A source whose release is unconfirmed may still hold its device, so
    /// no new session can be trusted to open it.
    const UNRELEASED: Self = Self::Faulted(SessionPhase::SourceUnreleased);

    /// What a session's end leaves, from what its thread reported.
    fn left_by(end: &SessionEnd) -> Self {
        match &end.left {
            _ if end.released.is_err() => Self::UNRELEASED,
            Ok(IdleMode::Native) => Self::Effects,
            // An idle after the join that did not return in time.
            Err(Error::Faulted(phase)) => Self::Faulted(*phase),
            Ok(IdleMode::Off) | Err(_) => Self::Stopped,
        }
    }

    /// What a session thread that did not report its end leaves.
    fn failed(error: &Error) -> Self {
        match error {
            Error::Faulted(phase) => Self::Faulted(*phase),
            // The thread died without reporting, so its source may still be held.
            _ => Self::UNRELEASED,
        }
    }

    /// Which lights these are, without what a running session holds.
    pub(super) fn kind(&self) -> Kind {
        match self {
            Self::Stopped => Kind::Stopped,
            Self::Effects => Kind::Effects,
            Self::ScreenSync(_) => Kind::ScreenSync,
            Self::Unanswered { .. } => Kind::Unanswered,
            Self::Faulted(phase) => Kind::Faulted(*phase),
        }
    }
}

/// The transitions: what each happening leaves the lights in. They decide;
/// the engine ([`Core`](super::Core)) does what they say, in order: calls
/// the adapters, waits, publishes and replies.
impl Lighting {
    /// What a session's end leaves, and the target it leaves when that
    /// changes. An ended source is completion: a `ScreenSync` target becomes
    /// `Off`, so lighting does not restart (a file would replay) until asked
    /// again; any other target was set by a command, and stands.
    pub(super) fn ended(
        end: &Result<SessionEnd>,
        target: LightingTarget,
    ) -> (Self, Option<LightingTarget>) {
        let (lighting, ended) = match end {
            Ok(end) => (Self::left_by(end), matches!(end.exit, Exit::Ended)),
            Err(error) => (Self::failed(error), false),
        };
        let complete = ended && target == LightingTarget::ScreenSync;
        (lighting, complete.then_some(LightingTarget::Off))
    }

    /// What stopping the running session for `mode` leaves, from what the
    /// stop reported: an exit other than [`Exit::Stopped`] ended on its own.
    pub(super) fn stopped(mode: IdleMode, stop: Result<SessionEnd>) -> Stopped {
        match stop {
            Err(error) => Stopped::Failed {
                lighting: Self::failed(&error),
                error,
            },
            Ok(end) if matches!(end.exit, Exit::Stopped) => Stopped::Idled {
                lighting: Self::left_by(&end),
                reply: end.result(),
            },
            Ok(end) if mode == IdleMode::Native => Stopped::ReplacedByEffects(end),
            Ok(end) => Stopped::Ended(end),
        }
    }

    /// After a running session's stop for a restart, whether the new session
    /// starts: only from `Stopped` with the target still `ScreenSync`. So it
    /// starts even when idling the old output failed (that device may be the
    /// one being replaced), but not over a source whose release is
    /// unconfirmed, nor after a source that had ended on its own: that
    /// turned the target `Off`.
    pub(super) fn restarts(&self, target: LightingTarget) -> bool {
        target == LightingTarget::ScreenSync && matches!(self, Self::Stopped)
    }

    /// What a session that could not start leaves, from its source's
    /// release: the lights as they were, unless the release is unconfirmed.
    pub(super) fn not_started(released: &Result<()>) -> Option<Self> {
        released.is_err().then_some(Self::UNRELEASED)
    }

    /// Whether a command for `target` switches these lights off again,
    /// instead of stepping towards it ([`Kind::towards`]): `Off` asked
    /// again while the published `failure` says they may not be off.
    pub(super) fn switches_off_again(
        &self,
        target: LightingTarget,
        failure: Option<&Error>,
    ) -> bool {
        target == LightingTarget::Off && self.may_not_be_off(failure).is_some()
    }

    /// The published `failure`, when it says these lights may not be off:
    /// no session, and the failure is the output's.
    pub(super) fn may_not_be_off<'a>(&self, failure: Option<&'a Error>) -> Option<&'a Error> {
        if !matches!(self, Self::Stopped) {
            return None;
        }
        failure.filter(|failure| failure.section() == Some(Section::Output))
    }

    /// The one rule for lights that do not answer: a request putting them in
    /// `mode` that failed only because they did not answer it is waited for
    /// instead of failing when all of these hold.
    pub(super) fn waits_for(
        error: &Error,
        mode: IdleMode,
        left: &Self,
        target: LightingTarget,
        shutting_down: bool,
    ) -> bool {
        let unanswered = error.escalation() == Escalation::Unanswered;
        // `Off`, or `Native` for `Effects`: never a mode the target left.
        let targets_own_mode = idle_mode(target) == Some(mode);
        // The request left them without a session or a fault.
        let from_stopped = matches!(left, Self::Stopped);
        // A shutdown waits for nothing.
        unanswered && targets_own_mode && from_stopped && !shutting_down
    }

    /// What a request putting these lights in `mode` leaves at `now`: what
    /// it `left` and its `reply`, or a wait for the lights by
    /// [`Self::waits_for`], tried again every [`UNANSWERED_INTERVAL`]. These
    /// lights hold no session: a stop takes it out first.
    pub(super) fn after_idle(
        &self,
        mode: IdleMode,
        left: Self,
        reply: Result<()>,
        target: LightingTarget,
        shutting_down: bool,
        now: Instant,
    ) -> Idled {
        let error = match reply {
            Err(error) if Self::waits_for(&error, mode, &left, target, shutting_down) => error,
            reply => {
                let answered = match (self, &reply) {
                    (Self::Unanswered { since, .. }, Ok(())) => Some(now.duration_since(*since)),
                    _ => None,
                };
                return Idled::Left {
                    lighting: left,
                    reply,
                    answered,
                };
            }
        };
        let again = now + UNANSWERED_INTERVAL;
        match *self {
            Self::Unanswered {
                mode: waiting,
                since,
                ..
            } if waiting == mode => Idled::WaitGoesOn {
                lighting: Self::Unanswered { mode, since, again },
                error,
            },
            _ => Idled::WaitBegins {
                lighting: Self::Unanswered {
                    mode,
                    since: now,
                    again,
                },
                error,
            },
        }
    }
}

/// Which lights are shown ([`Lighting::kind`]): what the step towards a
/// target depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Stopped,
    Effects,
    ScreenSync,
    Unanswered,
    Faulted(SessionPhase),
}

impl Kind {
    /// The step from these lights towards `target`. `restart`: a running
    /// session's adapters changed, so it is replaced; `output_changed`: the
    /// output may be another device, so an output showing its effects is
    /// put in that mode again.
    pub(super) fn towards(
        self,
        target: LightingTarget,
        restart: bool,
        output_changed: bool,
    ) -> Step {
        use LightingTarget as Target;
        match (target, self) {
            // Requests are refused before they get here (see
            // `Core::refusal`); this keeps the match total.
            (_, Self::Faulted(phase)) => Step::Faulted(phase),
            // Same path as Off followed by ScreenSync.
            (Target::ScreenSync, Self::ScreenSync) if restart => Step::Restart,
            (Target::ScreenSync, Self::ScreenSync) => Step::Stay,
            // A wait for the lights ends: the session's output is its own.
            (Target::ScreenSync, Self::Unanswered) => Step::Start { ends_wait: true },
            // Also from `Effects`: a start that failed from the device's own
            // effects leaves them showing, and the target says `ScreenSync`.
            (Target::ScreenSync, _) => Step::Start { ends_wait: false },
            (Target::Effects, Self::ScreenSync) => Step::Stop(IdleMode::Native),
            (Target::Effects, Self::Effects) if !output_changed => Step::Stay,
            // Also while waiting for the lights: they are asked again now.
            (Target::Effects, _) => Step::Idle(IdleMode::Native),
            (Target::Off, Self::ScreenSync) => Step::Stop(IdleMode::Off),
            // Other lights end a wait as they end a failure that says the
            // lights may not be off: they were never touched.
            (Target::Off, Self::Unanswered) if output_changed => Step::EndWait,
            (Target::Off, Self::Effects | Self::Unanswered) => Step::Idle(IdleMode::Off),
            // A command that does nothing about the output keeps a failure
            // that says the lights may not be off: it is still true. Other
            // lights make it no longer so.
            (Target::Off, Self::Stopped) if !output_changed => Step::MayNotBeOff,
            (Target::Off, Self::Stopped) => Step::Stay,
        }
    }
}

/// A step towards the target ([`Kind::towards`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// Nothing to do: the reply is success.
    Stay,
    /// The engine is faulted: the reply is the fault.
    Faulted(SessionPhase),
    /// Start a session; `ends_wait`: stop waiting for the lights first.
    Start { ends_wait: bool },
    /// Stop the running session and start a new one ([`Lighting::restarts`]).
    Restart,
    /// Stop the running session for this mode.
    Stop(IdleMode),
    /// Put the output in this mode without a session.
    Idle(IdleMode),
    /// Stop waiting for the lights: the reply is success.
    EndWait,
    /// Reply with the failure that says the lights may not be off, if one is
    /// published ([`Lighting::may_not_be_off`]).
    MayNotBeOff,
}

/// What stopping a running session left ([`Lighting::stopped`]).
pub(super) enum Stopped {
    /// The thread did not report its end: publish `lighting`, reply `error`.
    Failed { lighting: Lighting, error: Error },
    /// It stopped as asked: `lighting` and `reply`, by the rule for lights
    /// that do not answer ([`Lighting::after_idle`]).
    Idled {
        lighting: Lighting,
        reply: Result<()>,
    },
    /// It ended on its own as the lights' own effects were asked for: the
    /// effects replace what it ended with ([`replaced_by_effects`]), then as
    /// `Idled`.
    ReplacedByEffects(SessionEnd),
    /// It ended on its own while stopped for `Off`: reported as it ended
    /// ([`Lighting::ended`]); what the output was left in is the end's.
    Ended(SessionEnd),
}

/// What a request putting the lights in a mode left ([`Lighting::after_idle`]).
pub(super) enum Idled {
    /// Publish `lighting` and reply `reply`; `answered`: how long lights
    /// that were waited for took to answer.
    Left {
        lighting: Lighting,
        reply: Result<()>,
        answered: Option<Duration>,
    },
    /// The lights did not answer (`error`): a wait begins. Publish
    /// `lighting`; the reply is success.
    WaitBegins { lighting: Lighting, error: Error },
    /// They still do not answer: `lighting`, the next try moved, replaces
    /// the lights unpublished; the reply is success.
    WaitGoesOn { lighting: Lighting, error: Error },
}

/// The session and the measurement of its frame rate.
pub(super) struct Running {
    pub(super) session: Session,
    pub(super) meter: Meter,
}

/// What a running session needs from the engine.
pub(super) enum Event {
    Ended(Result<SessionEnd>),
    Measure,
}

impl Running {
    /// The session ended on its own, or it is time to measure. Cancel-safe.
    async fn next(&mut self) -> Event {
        tokio::select! {
            end = self.session.finished() => Event::Ended(end),
            () = self.meter.tick() => Event::Measure,
        }
    }
}

pub(super) async fn next(lighting: &mut Lighting) -> Event {
    match lighting {
        Lighting::ScreenSync(running) => running.next().await,
        Lighting::Stopped
        | Lighting::Effects
        | Lighting::Unanswered { .. }
        | Lighting::Faulted(_) => std::future::pending().await,
    }
}

/// The mode the lights are put in, without a session, for `target`.
fn idle_mode(target: LightingTarget) -> Option<IdleMode> {
    match target {
        LightingTarget::Off => Some(IdleMode::Off),
        LightingTarget::Effects => Some(IdleMode::Native),
        LightingTarget::ScreenSync => None,
    }
}

/// A session that ended on its own while being stopped for the lights' own
/// effects: what it leaves, and the reply. The effects replace what the
/// session ended with, so its failure is logged, not published; the reply
/// is whether the effects came on, unless the source's release is
/// unconfirmed.
fn replaced_by_effects(end: SessionEnd) -> (Lighting, Result<()>) {
    let lighting = Lighting::left_by(&end);
    if let Exit::Failed(error) = &end.exit {
        tracing::warn!(
            section = ?error.section(),
            error = %prysm_ports::chain(error),
            "the session failed on its own as the lights' own effects replaced it"
        );
    }
    let reply = end.released.and(end.left.map(drop));
    (lighting, reply)
}
