//! The shutdown sequence, and the blocking calls it may abandon.
use super::lighting::Lighting;
use super::{Command, Core, Engine};
use prysm_ports::{Error, Escalation, IdleMode, Result};
use tokio::sync::oneshot;
use tokio::time::Instant;

impl Engine {
    /// Stop the session (output idles `Off`), join it and end the task;
    /// while the lights are waited for, try their request once more instead.
    /// Lights that do not answer that last request are logged, not failed.
    /// Requests still queued, and any sent from now on, are refused with
    /// [`Error::Closed`] in their order, so none starts what shutdown stops.
    /// Blocking adapter or settings calls still running when the shutdown
    /// grace has passed since this call (or [`Self::begin_shutdown`]) are
    /// abandoned and left running on their threads; later ones fail at once.
    ///
    /// # Errors
    /// The engine is faulted, stopping the session failed other than by
    /// lights that do not answer, or a blocking call was abandoned.
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
            // One last try, waiting for nothing.
            Lighting::Unanswered { mode, .. } => {
                let tried = self.idle(mode).await;
                self.set_lighting(Lighting::left_by_idle(mode, &tried));
                tried
            }
            _ => self.stop(IdleMode::Off).await,
        };
        // Lights that do not answer are away, which fails no shutdown.
        let stopped = stopped.or_else(|error| {
            if error.escalation() != Escalation::Unanswered {
                return Err(error);
            }
            tracing::warn!(
                error = %prysm_ports::chain(&error),
                "the lights did not answer the last request; they may be left as they were"
            );
            Ok(())
        });
        let result = stopped.and(self.abandoned.take().map_or(Ok(()), Err));
        self.record(&result);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::{
        CallLog, FakeIdle, FakeOutputSettings, FakeOutputs, FakeSourceSettings, FakeSources,
        MemoryStore,
    };
    use crate::leds::UNANSWERED_INTERVAL;
    use prysm_ports::{AdapterSettings, LightingTarget, Registry, Settings};
    use std::time::Duration;

    /// A try due after shutdown began is the shutdown's last one, made once
    /// by it: the loop's timer was set before. Under the paused clock.
    #[tokio::test(start_paused = true)]
    async fn a_try_due_once_shutdown_began_is_the_shutdowns() {
        let log = CallLog::new();
        let output = FakeOutputSettings {
            name: "o".into(),
            idle: FakeIdle::UnansweredUntil("never".into()),
            ..Default::default()
        };
        let settings = Settings {
            capture: Some(AdapterSettings::new("fake", &FakeSourceSettings::default())),
            output: Some(AdapterSettings::new("fake", &output)),
            ..Settings::default()
        };
        let mut registry = Registry::default();
        registry.add_source(FakeSources(log.clone()));
        registry.add_output(FakeOutputs(log.clone()));
        let store = Box::new(MemoryStore::with(settings));
        let (handle, engine) = Engine::start(registry, store, Duration::from_secs(2))
            .await
            .unwrap();
        let engine = engine.shutdown_grace(UNANSWERED_INTERVAL * 10);
        handle.set_target(LightingTarget::Effects).await.unwrap();
        engine.begin_shutdown();
        tokio::time::advance(UNANSWERED_INTERVAL).await;
        // A round trip through the loop after what was due.
        assert!(handle.set_calibration(None).await.is_err());
        assert_eq!(log.count("output.idle:o:Native"), 1, "left to the shutdown");
        engine.shutdown().await.unwrap();
        assert_eq!(log.count("output.idle:o:Native"), 2, "its one last try");
    }
}
