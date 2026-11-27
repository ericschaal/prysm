//! Drive the exclusive Screen sync owner through startup and confirmed release.
use super::Event;
use crate::engine::Core;
use crate::meter::Meter;
use crate::session::{Lease, Notice, Session, SessionClaims, SessionEnd, Start};
use crate::session_lifecycle::{OutputRelease, SessionPhase};
use crate::worker::Exit;
use prysm_ports::{Availability, Error, Power, Result, Section, SessionPhase as FaultPhase};
use tokio::time::Instant;

impl Core {
    pub(super) fn stop_session(&mut self, mode: OutputRelease, end_request: bool) {
        if self.lighting.lifecycle.stop(mode, end_request) {
            self.lighting.meter = None;
            if let Some(session) = &mut self.lighting.active {
                self.lighting.capture.teardown(&mut session.claims.capture);
                self.lighting.output.teardown(&mut session.claims.output);
                session.request_stop();
                self.lighting.stopping_at = Some(Instant::now() + self.stop_deadline);
            }
        }
    }

    /// Returns true when the attempt ended and its mode exit still needs dispatch.
    pub(super) fn drive_sync(&mut self) -> bool {
        if self.lighting.intent.sync_open_intent().is_none() {
            return false;
        }
        let capture = self.lighting.capture.available();
        let output = self.lighting.output.available();
        if [capture, output].contains(&Availability::Blocked) {
            if self.lighting.active.is_none() {
                self.stop_sync_intent();
                return true;
            } else if !self.lighting.lifecycle.stopping() {
                self.stop_session(OutputRelease::Automatic, true);
            }
            return false;
        }
        if !self.lighting.capture.can_retry_open() || !self.lighting.output.can_retry_open() {
            return false;
        }
        let (Ok(capture_port), Ok(output_port)) =
            (&self.lighting.capture.port, &self.lighting.output.port)
        else {
            return false;
        };
        if !self.lighting.lifecycle.start(capture, output) {
            return false;
        }
        let capture = capture_port.clone();
        let output = output_port.clone();
        let Some(claim) = self.lighting.capture.claim() else {
            self.lighting.lifecycle.start_failed();
            return false;
        };
        let session = Session::start(
            capture,
            claim,
            Start {
                output,
                config: self.config(),
                feeds: self.feeds.clone(),
                stats: self.stats.clone(),
            },
        );
        match session {
            Ok(session) => self.lighting.active = Some(session),
            Err((error, claim)) => {
                let mut claim = Lease::Claimed(claim);
                self.lighting.failure = Some(error);
                self.lighting.capture.teardown(&mut claim);
                if !self.lighting.capture.release(claim) {
                    self.session_fault(Error::Faulted(FaultPhase::SourceUnreleased));
                    return true;
                }
                self.lighting.lifecycle.start_failed();
                self.stop_sync_intent();
                return true;
            }
        }
        false
    }

    pub(super) async fn session_notice(&mut self, notice: Notice) {
        match notice {
            Notice::CaptureOpened if self.lighting.lifecycle.capture_opened() => {
                if let Some(intent) = self.lighting.intent.sync_open_intent()
                    && self.lighting.output.available() == Availability::Ready
                    && let Some(session) = &mut self.lighting.active
                    && let Some(claim) = self.lighting.output.claim()
                {
                    session.open_output(intent, claim);
                } else {
                    self.stop_session(OutputRelease::Automatic, false);
                }
            }
            Notice::OutputOpened if self.lighting.lifecycle.output_opened() => {
                self.lighting.failure = None;
                self.lighting.meter = Some(Meter::new(&self.stats));
                self.lighting.intent.sync_started();
            }
            Notice::CaptureOpened | Notice::OutputOpened => {}
            Notice::Observed(observation) => {
                if self.lighting.lifecycle.phase() == SessionPhase::Running {
                    self.lighting.intent.observe_power(Some(observation.power));
                }
            }
            Notice::Handoff(observation) => {
                if !self.lighting.lifecycle.stopping() {
                    self.lighting.intent.observe_power(Some(observation.power));
                    self.lighting.intent.handed_off();
                }
            }
            Notice::Ended(exit) if !self.lighting.lifecycle.stopping() => match exit {
                Exit::Ended => self.stop_session(OutputRelease::Off, true),
                Exit::Stopped => self.stop_session(OutputRelease::Preserve, true),
                Exit::Failed(error) => {
                    self.lighting.failure = Some(error.clone());
                    match error.section() {
                        Some(section @ (Section::Capture | Section::Output)) => {
                            if section == Section::Capture {
                                self.lighting.capture.failed(error);
                            } else {
                                self.lighting.output.failed(error);
                            }
                            match self.lighting.lifecycle.phase() {
                                SessionPhase::OpeningCapture => self.lighting.capture.failed_open(),
                                SessionPhase::OpeningOutput if section == Section::Capture => {
                                    self.lighting.capture.failed_open()
                                }
                                SessionPhase::OpeningOutput => self.lighting.output.failed_open(),
                                _ => {}
                            }
                            self.stop_session(OutputRelease::Off, false);
                        }
                        _ => {
                            let unstable = self.lighting.lifecycle.phase() != SessionPhase::Running;
                            self.stop_session(OutputRelease::Off, unstable);
                        }
                    }
                }
            },
            Notice::Ended(_) => {}
            Notice::IdleReady { capture_released } => {
                if !capture_released && self.lighting.intent.off_pending() {
                    // The upcoming fault will prevent a separate command from
                    // running. The owner can still perform Off after close.
                    self.stop_session(OutputRelease::Off, true);
                }
                if let Some(mode) = self.lighting.lifecycle.take_idle()
                    && let Some(session) = &mut self.lighting.active
                {
                    session.idle(mode, self.lighting.intent.physical_power());
                }
            }
            Notice::Finished(end) => {
                if let Some(active) = self.lighting.active.take() {
                    let output_is_current = active.output_is_current();
                    let claims = active.join().await;
                    self.session_finished(end, output_is_current, claims);
                }
            }
        }
    }

    fn session_finished(
        &mut self,
        result: Result<SessionEnd>,
        output_is_current: bool,
        claims: SessionClaims,
    ) {
        self.lighting.stopping_at = None;
        let end = match result {
            Ok(end) => end,
            Err(error) => {
                self.session_fault(error);
                return;
            }
        };
        if let Err(error) = end.capture {
            tracing::warn!(error = %prysm_ports::chain(&error), "capture release was not confirmed");
            self.session_fault(Error::Faulted(FaultPhase::SourceUnreleased));
            return;
        }
        if !self.lighting.capture.release(claims.capture) {
            self.session_fault(Error::Faulted(FaultPhase::SourceUnreleased));
            return;
        }
        if let Err(error @ Error::Faulted(_)) = end.output {
            self.session_fault(error);
            return;
        }
        if !self.lighting.output.release(claims.output) {
            self.session_fault(Error::Faulted(FaultPhase::IdlingOutput));
            return;
        }
        if output_is_current && end.mode == OutputRelease::Off {
            self.lighting
                .intent
                .observe_power(end.output.is_ok().then_some(Power::Off));
        }
        if let Err(error) = end.output
            && output_is_current
        {
            self.lighting.output.failed(error.clone());
            self.lighting.failure.get_or_insert(error);
        }
        let end_request = self.lighting.lifecycle.released();
        if self.lighting.intent.exiting() {
            self.lighting.intent.exited();
            self.lighting.failure = None;
        } else if end_request {
            self.stop_sync_intent();
        }
    }

    pub(super) fn session_fault(&mut self, error: Error) {
        tracing::warn!(error = %prysm_ports::chain(&error), "session release could not be confirmed");
        self.lighting.active = None;
        self.lighting.meter = None;
        self.lighting.stopping_at = None;
        let phase = match &error {
            Error::Faulted(phase) => *phase,
            _ => FaultPhase::SourceUnreleased,
        };
        self.lighting.intent.fault(error);
        self.lighting.lifecycle.fault(phase);
    }

    pub(in crate::engine) async fn shutdown_lighting(&mut self) -> Result<()> {
        self.lighting.capture.cancel();
        self.lighting.output.cancel();
        // Pending requests are retired. A sent power command still owns the
        // output until it returns, within the host's shutdown grace.
        self.lighting.intent.pause();
        self.lighting.failure = None;
        self.stop_session(OutputRelease::Off, true);
        let until = self
            .abandon_at
            .borrow()
            .unwrap_or_else(|| Instant::now() + self.stop_deadline);
        while self.lighting.active.is_some() || self.lighting.command.is_some() {
            tokio::select! {
                event = self.lighting.next() => match event {
                    Event::Session(notice) => self.session_notice(notice).await,
                    Event::StopTimedOut => {
                        let phase = self.lighting.active.as_ref().map_or(FaultPhase::Finishing, Session::deadline_phase);
                        self.session_fault(Error::Faulted(phase));
                    }
                    Event::Power(result) => self.power_finished(result),
                    _ => {}
                },
                () = tokio::time::sleep_until(until), if self.lighting.command.is_some() => {
                    if let Some(session) = &self.lighting.active {
                        self.session_fault(Error::Faulted(session.deadline_phase()));
                    } else {
                        self.lighting.command = None;
                        self.abandoned = Some(Error::other("an output command did not finish before shutdown"));
                    }
                    break;
                }
            }
        }
        self.publish_lighting();
        match self.lighting.fault() {
            Some(phase) => Err(Error::Faulted(phase)),
            None => match self.lighting.failure.clone() {
                Some(error) if error.escalation() != prysm_ports::Escalation::Unanswered => {
                    Err(error)
                }
                _ => Ok(()),
            },
        }
    }
}
