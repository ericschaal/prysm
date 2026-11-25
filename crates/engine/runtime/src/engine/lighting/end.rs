//! A session that ended on its own: what its end leaves, published, and
//! the failures its result does not say.
use super::Lighting;
use super::publish::log_unpublished;
use crate::engine::Core;
use crate::session::SessionEnd;
use crate::worker::Exit;
use prysm_ports::{Error, IdleMode, Result};

impl Core {
    /// Handle the end of a session whose loop already ended on its own, as
    /// if the end had come before what is being handled now: a request or
    /// shutdown then meets what the end left, whichever arrived first. A
    /// session still running when the request arrives is the request's.
    pub(in crate::engine) async fn settle(&mut self) {
        if !matches!(&self.lighting, Lighting::ScreenSync(running) if running.session.has_ended()) {
            return;
        }
        let Some(running) = self.take_running() else {
            return;
        };
        // The thread is releasing what it holds: wait for it within the
        // stop deadline. `Off` is the mode it leaves the output in.
        let end = running
            .session
            .stop(IdleMode::Off, self.stop_deadline)
            .await;
        let result = self.ended(end);
        log_unpublished(&self.lighting, &result);
    }

    /// The session ended on its own and reported it.
    pub(in crate::engine) async fn session_ended(&mut self, end: Result<SessionEnd>) {
        if let Some(running) = self.take_running() {
            running.session.join().await;
        }
        let result = self.ended(end);
        log_unpublished(&self.lighting, &result);
    }

    /// A session ended on its own: a failure, or its source ended; seen when
    /// it ended, or by a stop that came after. Publish what it left with its
    /// failure, and return that failure; with the target it leaves
    /// ([`Lighting::ended`]): a changed target ends calibration.
    pub(in crate::engine) fn ended(&mut self, end: Result<SessionEnd>) -> Result<()> {
        let (lighting, target) = Lighting::ended(&end, self.state.borrow().target);
        // The result is the first failure: a release or an idle that failed
        // after it is said only here.
        for later in end.iter().flat_map(hidden) {
            tracing::warn!(
                error = %prysm_ports::chain(later),
                "a session that had already failed also failed to end cleanly"
            );
        }
        let result = end.and_then(SessionEnd::result);
        let failure = result.as_ref().err().cloned();
        self.set_lighting_and(lighting, failure, |s| {
            if let Some(target) = target {
                s.target = target;
            }
        });
        if target.is_some() {
            self.calibrate(None);
        }
        result
    }
}

/// The failures a session end's result does not carry: every one after the
/// first ([`SessionEnd::result`]). A fault is not one: the state carries
/// its phase.
pub(super) fn hidden(end: &SessionEnd) -> impl Iterator<Item = &Error> {
    let exit = match &end.exit {
        Exit::Failed(failure) => Some(failure),
        Exit::Stopped | Exit::Ended => None,
    };
    exit.into_iter()
        .chain(end.released.as_ref().err())
        .chain(end.left.as_ref().err())
        .skip(1)
        .filter(|error| !matches!(error, Error::Faulted(_)))
}
