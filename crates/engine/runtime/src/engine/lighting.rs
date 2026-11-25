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

use crate::leds::UNREACHABLE;
use crate::meter::Meter;
use crate::session::{Session, SessionEnd};
use crate::worker::Exit;
use prysm_ports::{
    DeviceReport, Error, IdleMode, LightingTarget, Result, SessionPhase, SessionState,
};
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
    /// ([`Core::idled`](super::Core::idled)). Their mode is not known meanwhile.
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
