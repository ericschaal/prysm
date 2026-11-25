//! The shutdown sequence, and the blocking calls it may abandon.
use super::lighting::Lighting;
use super::{Command, Core, Engine};
use prysm_ports::{Error, IdleMode, Result};
use tokio::sync::oneshot;
use tokio::time::Instant;

impl Engine {
    /// Stop the session (output idles `Off`), join it and end the task;
    /// while the lights are waited for, try their request once more instead.
    /// Requests still queued, and any sent from now on, are refused with
    /// [`Error::Closed`] in their order, so none starts what shutdown stops.
    /// Blocking adapter or settings calls still running when the shutdown
    /// grace has passed since this call (or [`Self::begin_shutdown`]) are
    /// abandoned and left running on their threads; later ones fail at once.
    ///
    /// # Errors
    /// The engine is faulted, stopping the session failed, or a blocking
    /// call was abandoned.
    pub async fn shutdown(self) -> Result<()> {
        self.begin_shutdown();
        let (reply, result) = oneshot::channel();
        let sent = self.commands.send(Command::Shutdown(reply)).await;
        let result = match sent {
            Ok(()) => result.await.unwrap_or(Err(Error::Closed)),
            Err(_) => Err(Error::Closed),
        };
        let _ = self.task.await;
        result
    }

    /// What [`Self::shutdown`] starts with: from the first call, queued
    /// requests are refused and the shutdown grace runs; the engine still
    /// takes commands such as surface failures until the shutdown itself.
    pub(crate) fn begin_shutdown(&self) {
        let at = Instant::now() + self.shutdown_grace;
        self.abandon_at.send_if_modified(|abandon_at| {
            let first = abandon_at.is_none();
            if first {
                *abandon_at = Some(at);
            }
            first
        });
    }
}

/// Run blocking adapter code (open, idle, store I/O) off the async runtime.
pub(super) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| Error::Panicked("blocking adapter call"))
}

impl Core {
    /// Run blocking adapter or store code off the async runtime. A call still
    /// running at the shutdown's one deadline is abandoned, left running on
    /// its thread; once one was, later calls fail without starting. So calls
    /// that never return cannot keep the engine from the session's ordered
    /// stop.
    ///
    /// # Errors
    /// The call panicked, or was abandoned or not started.
    pub(super) async fn run_blocking<T: Send + 'static>(
        &mut self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T> {
        if let Some(abandoned) = &self.abandoned {
            return Err(abandoned.clone());
        }
        let mut abandon_at = self.abandon_at.clone();
        let abandon = async move {
            let at = abandon_at
                .wait_for(Option::is_some)
                .await
                .ok()
                .and_then(|at| *at);
            match at {
                Some(at) => tokio::time::sleep_until(at).await,
                // The engine was dropped without a shutdown: nothing waits.
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            done = blocking(work) => done,
            () = abandon => {
                let error = Error::other(
                    "an adapter or settings call did not return within the shutdown \
                     grace, so it was abandoned",
                );
                self.abandoned = Some(error.clone());
                Err(error)
            }
        }
    }

    pub(super) async fn shutdown(&mut self) -> Result<()> {
        self.settle().await;
        // Calibration ends at shutdown even on a faulted engine: the last
        // snapshot shows no pattern that nothing is showing any more.
        self.calibrate(None);
        if let Lighting::Faulted(phase) = self.lighting {
            return Err(Error::Faulted(phase));
        }
        let stopped = match self.lighting {
            // One last try, waiting for nothing: lights that do not answer
            // it fail the shutdown, as any other failed idle does.
            Lighting::Unanswered { mode, .. } => {
                let tried = self.idle(mode).await;
                if let Err(error) = &tried {
                    tracing::warn!(
                        error = %prysm_ports::chain(error),
                        "the last try for the lights at shutdown failed; they are left as they are"
                    );
                }
                self.set_lighting(Lighting::idled(mode, &tried));
                tried
            }
            _ => self.stop(IdleMode::Off).await,
        };
        let result = stopped.and(self.abandoned.take().map_or(Ok(()), Err));
        self.record(&result);
        result
    }
}
