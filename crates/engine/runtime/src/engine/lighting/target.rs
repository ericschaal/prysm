//! Driving the lights towards the target: starting and stopping sessions,
//! and putting the output in a mode without one.
use super::{Lighting, Running, Step, Stopped, replaced_by_effects};
use crate::engine::Core;
use crate::leds::{Configured, Device};
use crate::meter::Meter;
use crate::session::Session;
use prysm_ports::{Error, IdleMode, LightingTarget, Result, Section};

impl Core {
    pub(in crate::engine) async fn set_target(&mut self, target: LightingTarget) -> Result<()> {
        self.state.send_modify(|s| s.target = target);
        if target != LightingTarget::ScreenSync {
            self.calibrate(None);
        }
        let off_again = self
            .lighting
            .switches_off_again(target, self.state.borrow().session.failure());
        let result = if off_again {
            self.idle_output(IdleMode::Off).await
        } else {
            self.reconcile(false, false).await
        };
        self.record(&result);
        result
    }

    /// The published failure, when it says the lights may not be off
    /// ([`Lighting::may_not_be_off`]).
    fn off_may_not_be_off(&self) -> Option<Error> {
        self.lighting
            .may_not_be_off(self.state.borrow().session.failure())
            .cloned()
    }

    /// Drive the session towards the target. A target that is not reached
    /// (a failed start or idle) is attempted again at the next call, so every
    /// settings save retries it. `restart` replaces a running session (its
    /// adapters changed); `output_changed` puts an output showing its effects
    /// in that mode again (it may be another device).
    pub(in crate::engine) async fn reconcile(
        &mut self,
        restart: bool,
        output_changed: bool,
    ) -> Result<()> {
        let target = self.state.borrow().target;
        match self
            .lighting
            .kind()
            .towards(target, restart, output_changed)
        {
            Step::Stay => Ok(()),
            Step::Faulted(phase) => Err(Error::Faulted(phase)),
            Step::Restart => {
                let stopped = self.stop(IdleMode::Off).await;
                if !self.lighting.restarts(self.state.borrow().target) {
                    return stopped;
                }
                // What the old session's stop left is replaced by the new
                // session: its failure is logged, not published.
                if let Err(error) = &stopped {
                    tracing::warn!(
                        section = ?error.section(),
                        error = %prysm_ports::chain(error),
                        "the replaced session did not stop cleanly"
                    );
                }
                let device = self.lighting.shows(stopped.as_ref().err());
                self.start(device).await
            }
            Step::Start { ends_wait } => {
                let device = self.lighting.shows(self.state.borrow().session.failure());
                if ends_wait {
                    self.set_lighting(Lighting::Stopped);
                }
                self.start(device).await
            }
            Step::Stop(mode) => self.stop(mode).await,
            Step::Idle(mode) => self.idle_output(mode).await,
            Step::EndWait => {
                self.set_lighting(Lighting::Stopped);
                Ok(())
            }
            Step::MayNotBeOff => self.off_may_not_be_off().map_or(Ok(()), Err),
        }
    }

    /// Open the source and start a session. The output is validated here
    /// but opened by the session at its first frame, so a source without
    /// frames never takes the device over; its end puts the output in the
    /// stop's mode unless it shows that mode already (`device`).
    async fn start(&mut self, device: Device) -> Result<()> {
        let (capture, output) = {
            let snapshot = self.state.borrow();
            let settings = &snapshot.settings;
            let capture = self.configured(settings.capture.as_ref(), Section::Capture)?;
            let output = self.configured(settings.output.as_ref(), Section::Output)?;
            self.registry.validate(settings)?;
            (capture, output)
        };
        // Outside the borrow above: `config` borrows the state itself.
        let config = self.config();
        let registry = self.registry.clone();
        let source = self
            .run_blocking(move || registry.open_source(&capture))
            .await?
            .map_err(|error| error.in_section(Section::Capture))?;
        self.stats.source_opened();
        let output = Configured::new(self.registry.clone(), output);
        let session = match Session::start(
            source,
            output,
            device,
            config,
            self.feeds.clone(),
            self.stats.clone(),
        ) {
            Ok(session) => session,
            Err((error, source)) => {
                // The session never ran: release the source here instead.
                let released = self
                    .run_blocking(move || source.close())
                    .await
                    .and_then(|closed| closed);
                if let Err(release) = &released {
                    // The fault says only its phase: its cause is logged.
                    tracing::warn!(
                        error = %prysm_ports::chain(release),
                        "the source of a session that could not start was not released"
                    );
                }
                if let Some(lighting) = Lighting::not_started(&released) {
                    self.set_lighting(lighting);
                }
                return Err(error);
            }
        };
        self.set_lighting(Lighting::ScreenSync(Running {
            session,
            meter: Meter::new(&self.stats),
        }));
        Ok(())
    }

    /// Stop the running session, if one runs: without one there is nothing
    /// to stop and the lighting stays as it is. A session that ended on its
    /// own after the request began (one that had ended before was settled
    /// first, see [`Self::settle`]) ended while being stopped. Stopped for
    /// `Off`, its end is published ([`Self::ended`]) and the reply is its
    /// failures in order, then the output's: with the lights off, they say
    /// why the lights may not be. Stopped for the lights' own effects, the
    /// effects replace what it ended with, which is logged: the reply is
    /// the output's idle (see [`replaced_by_effects`]). An unconfirmed
    /// release wins the state, then an idle that did not return in time.
    /// Stopped as asked, or for the lights' own effects, an idle the lights
    /// did not answer is waited for ([`Self::idled`]); a session that ended
    /// on its own while stopped for `Off` is reported as it ended.
    pub(in crate::engine) async fn stop(&mut self, mode: IdleMode) -> Result<()> {
        let Some(running) = self.take_running() else {
            return Ok(());
        };
        let stop = running.session.stop(mode, self.stop_deadline).await;
        match Lighting::stopped(mode, stop) {
            Stopped::Failed { lighting, error } => {
                self.set_lighting(lighting);
                Err(error)
            }
            Stopped::Idled { lighting, reply } => self.idled(mode, lighting, reply),
            Stopped::ReplacedByEffects(end) => {
                let (lighting, reply) = replaced_by_effects(end);
                self.idled(mode, lighting, reply)
            }
            Stopped::Ended(end) => self.ended(Ok(end)),
        }
    }

    /// Effects have no session: put the output in `mode` without opening
    /// it, by the rule of [`Self::idled`].
    pub(in crate::engine) async fn idle_output(&mut self, mode: IdleMode) -> Result<()> {
        let result = self.idle(mode).await;
        self.idled(mode, Lighting::idled(mode, &result), result)
    }

    /// The request putting the output in `mode` without opening it; bounded
    /// by the adapter's own limit.
    pub(in crate::engine) async fn idle(&mut self, mode: IdleMode) -> Result<()> {
        let output = self.configured(
            self.state.borrow().settings.output.as_ref(),
            Section::Output,
        )?;
        let registry = self.registry.clone();
        self.run_blocking(move || registry.idle_output(&output, mode))
            .await?
            .map_err(|error| error.in_section(Section::Output))
    }
}
