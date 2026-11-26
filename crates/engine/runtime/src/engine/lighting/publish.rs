//! The only writer of the snapshot's lighting: its state, failure, input and
//! device report.
use super::{Lighting, Running};
use crate::engine::Core;
use prysm_ports::{Error, Input, Result, SessionPhase, Snapshot};

impl Core {
    /// Publish `lighting` without a failure: a command path, which then
    /// records its own ([`Self::record`]), so no earlier failure is shown
    /// beside the new state.
    pub(in crate::engine) fn set_lighting(&mut self, lighting: Lighting) {
        self.set_lighting_and(lighting, None, |_| {});
    }

    /// The only writer of the snapshot's `session`, with `failure` where it
    /// can carry one, publishing `change` in the same snapshot. It also
    /// resets `input` and `device` ([`Lighting::device`]), which
    /// [`Self::measure`] updates while a session runs.
    pub(in crate::engine) fn set_lighting_and(
        &mut self,
        lighting: Lighting,
        failure: Option<Error>,
        change: impl FnOnce(&mut Snapshot),
    ) {
        let session = lighting.state(failure);
        let input = matches!(lighting, Lighting::ScreenSync(_)).then(Input::default);
        let device = lighting.device();
        self.lighting = lighting;
        self.state.send_modify(|s| {
            s.session = session;
            s.input = input;
            s.device = device;
            change(s);
        });
    }

    /// The running session, taken out of the lighting, which is left
    /// `Stopped` (unpublished) until the caller sets what the end leaves.
    pub(in crate::engine) fn take_running(&mut self) -> Option<Running> {
        match std::mem::replace(&mut self.lighting, Lighting::Stopped) {
            Lighting::ScreenSync(running) => Some(running),
            other => {
                self.lighting = other;
                None
            }
        }
    }

    pub(in crate::engine) fn measure(&mut self) {
        let Lighting::ScreenSync(running) = &mut self.lighting else {
            return;
        };
        let input = Some(running.meter.sample(&self.stats));
        let device = running.session.observed();
        self.state.send_if_modified(|s| {
            let changed = s.input != input || s.device != device;
            s.input = input;
            s.device = device;
            changed
        });
    }

    /// Publish `result` as the session's failure. Without one, a store that
    /// could not be read, or saved settings that do not validate (loaded
    /// that way), stay reported until replaced. A fault carries no failure,
    /// so what ended in it is logged ([`unpublished`]). Running screen sync
    /// is reached only by a command that succeeded.
    pub(in crate::engine) fn record(&self, result: &Result<()>) {
        debug_assert!(
            !(result.is_err() && matches!(self.lighting, Lighting::ScreenSync(_))),
            "a failure beside running screen sync: {result:?}"
        );
        log_unpublished(&self.lighting, result);
        self.state.send_modify(|s| {
            let failure = match result {
                Err(error) => Some(error.clone()),
                Ok(()) => self
                    .unreadable
                    .clone()
                    .or_else(|| self.registry.validate(&s.settings).err()),
            };
            s.session = self.lighting.state(failure);
        });
    }
}

/// The error a fault leaves unpublished: what ended in it, when that is not
/// the fault itself. A fault's state carries only its phase.
pub(super) fn unpublished<'a>(
    lighting: &Lighting,
    result: &'a Result<()>,
) -> Option<(SessionPhase, &'a Error)> {
    match (lighting, result) {
        (Lighting::Faulted(phase), Err(error)) if !matches!(error, Error::Faulted(_)) => {
            Some((*phase, error))
        }
        _ => None,
    }
}

/// Log what [`unpublished`] finds, so a fault's cause is never lost.
pub(super) fn log_unpublished(lighting: &Lighting, result: &Result<()>) {
    if let Some((phase, error)) = unpublished(lighting, result) {
        tracing::warn!(
            %phase,
            error = %prysm_ports::chain(error),
            "lighting faulted after this failure"
        );
    }
}
