//! The wait for lights that do not answer the request putting them in the
//! target's mode, and its tries again.
use super::{Lighting, idle_mode};
use crate::engine::Core;
use crate::leds::{UNANSWERED_INTERVAL, log_answered, log_unanswered};
use prysm_ports::{Escalation, IdleMode, Result};
use tokio::time::Instant;

impl Core {
    /// Publish what a request putting the lights in `mode` left (`lighting`)
    /// and return its `reply`, by the one rule for lights that do not
    /// answer: when `mode` is the target's (`Off`, or `Native` for
    /// `Effects`) and the request failed only because the lights did not
    /// answer it ([`Escalation::Unanswered`]), that is no failure. The
    /// target stands, the device reports `Unreachable`, and the request is
    /// tried again every [`UNANSWERED_INTERVAL`] ([`Self::try_again`]) until
    /// it is answered or a command asks for something else; a wait that
    /// goes on is not published again. A shutdown waits for nothing, and
    /// every other failure is the reply.
    pub(in crate::engine) fn idled(
        &mut self,
        mode: IdleMode,
        lighting: Lighting,
        reply: Result<()>,
    ) -> Result<()> {
        let target = self.state.borrow().target;
        let shutting_down = self.abandon_at.borrow().is_some();
        let unanswered = match (&lighting, reply) {
            (Lighting::Stopped, Err(error))
                if error.escalation() == Escalation::Unanswered
                    && Some(mode) == idle_mode(target)
                    && !shutting_down =>
            {
                error
            }
            (_, reply) => {
                if let (Lighting::Unanswered { since, .. }, Ok(())) = (&self.lighting, &reply) {
                    log_answered(since.elapsed());
                }
                self.set_lighting(lighting);
                return reply;
            }
        };
        let again = Instant::now() + UNANSWERED_INTERVAL;
        match &mut self.lighting {
            Lighting::Unanswered {
                mode: waiting,
                again: next,
                ..
            } if *waiting == mode => {
                log_unanswered(&unanswered, false);
                *next = again;
            }
            _ => {
                log_unanswered(&unanswered, true);
                let since = Instant::now();
                self.set_lighting(Lighting::Unanswered { mode, since, again });
            }
        }
        Ok(())
    }

    /// When the request the lights did not answer is tried again; never once
    /// shutdown began, which makes the one last try.
    pub(in crate::engine) fn again(&self) -> Option<Instant> {
        match self.lighting {
            Lighting::Unanswered { again, .. } if self.abandon_at.borrow().is_none() => Some(again),
            _ => None,
        }
    }

    /// Try the request the lights did not answer again. While it runs,
    /// commands wait in the mailbox: one request, bounded by the adapter.
    pub(in crate::engine) async fn try_again(&mut self) {
        let Lighting::Unanswered { mode, .. } = self.lighting else {
            return;
        };
        let result = self.idle_output(mode).await;
        if !matches!(self.lighting, Lighting::Unanswered { .. }) {
            self.record(&result);
        }
    }
}
