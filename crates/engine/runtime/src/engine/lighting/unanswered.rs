//! The wait for lights that do not answer the request putting them in the
//! target's mode, and its tries again every
//! [`UNANSWERED_INTERVAL`](crate::UNANSWERED_INTERVAL). The engine serves
//! commands between tries, and those that come during one after it. Screen
//! sync ends the wait: its session waits for its own open. Another target,
//! or the same one or a settings save, tries at once in the target's mode,
//! and the wait goes on while the lights still do not answer; a saved
//! output begins a wait of its own. A shutdown makes one last try and waits for nothing.
use super::{Idled, Lighting};
use crate::engine::Core;
use crate::leds::{log_answered, log_unanswered};
use prysm_ports::{IdleMode, Result};
use tokio::time::Instant;

impl Core {
    /// Publish what a request putting the lights in `mode` left (`lighting`)
    /// and return its `reply`, by the one rule for lights that do not
    /// answer ([`Lighting::waits_for`]): that is no failure. The target
    /// stands, the device reports `Unreachable`, and the request is tried
    /// again ([`Self::try_again`]); a wait that goes on is not published
    /// again. Every other failure is the reply.
    pub(in crate::engine) fn idled(
        &mut self,
        mode: IdleMode,
        lighting: Lighting,
        reply: Result<()>,
    ) -> Result<()> {
        let target = self.state.borrow().target;
        let shutting_down = self.abandon_at.borrow().is_some();
        let now = Instant::now();
        match self
            .lighting
            .after_idle(mode, lighting, reply, target, shutting_down, now)
        {
            Idled::Left {
                lighting,
                reply,
                answered,
            } => {
                if let Some(after) = answered {
                    log_answered(after);
                }
                self.set_lighting(lighting);
                reply
            }
            Idled::WaitBegins { lighting, error } => {
                log_unanswered(&error, true);
                self.set_lighting(lighting);
                Ok(())
            }
            Idled::WaitGoesOn { lighting, error } => {
                log_unanswered(&error, false);
                self.lighting = lighting;
                Ok(())
            }
        }
    }

    /// When the request the lights did not answer is tried again; never once
    /// shutdown began, which makes the one last try.
    pub(in crate::engine) fn again(&self) -> Option<Instant> {
        match self.lighting {
            Lighting::Unanswered { again, .. } if self.abandon_at.borrow().is_none() => Some(again),
            _ => None,
        }
    }

    /// Try the request the lights did not answer again, if a try is still
    /// due ([`Self::again`]): the loop's timer may have been set before
    /// shutdown began. While it runs, commands wait in the mailbox: one
    /// request, bounded by the adapter.
    pub(in crate::engine) async fn try_again(&mut self) {
        let Lighting::Unanswered { mode, .. } = self.lighting else {
            return;
        };
        if self.again().is_none() {
            return;
        }
        let result = self.idle_output(mode).await;
        if !matches!(self.lighting, Lighting::Unanswered { .. }) {
            self.record(&result);
        }
    }
}
