//! The part of Screen sync which owns a session, from the first open to join.
//! Adapter availability and the requested lighting mode live with their owners.
use prysm_ports::{Availability, SessionPhase as FaultPhase};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SessionPhase {
    #[default]
    Idle,
    OpeningCapture,
    OpeningOutput,
    Running,
    Stopping {
        /// Taken when the worker asks to idle, after capture has closed.
        idle: Option<OutputRelease>,
        /// A finite source or an initialization crash ends the request after join.
        end_request: bool,
    },
    Faulted(FaultPhase),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputRelease {
    Automatic,
    Preserve,
    Off,
}

#[derive(Debug, Default)]
pub struct SessionLifecycle {
    phase: SessionPhase,
}

impl SessionLifecycle {
    pub fn phase(&self) -> SessionPhase {
        self.phase
    }

    pub fn start(&mut self, capture: Availability, output: Availability) -> bool {
        if self.phase != SessionPhase::Idle
            || capture != Availability::Ready
            || output != Availability::Ready
        {
            return false;
        }
        self.phase = SessionPhase::OpeningCapture;
        true
    }

    /// Roll back an attempt that never spawned a worker.
    pub fn start_failed(&mut self) {
        if self.phase == SessionPhase::OpeningCapture {
            self.phase = SessionPhase::Idle;
        }
    }

    pub fn capture_opened(&mut self) -> bool {
        if self.phase != SessionPhase::OpeningCapture {
            return false;
        }
        self.phase = SessionPhase::OpeningOutput;
        true
    }

    pub fn output_opened(&mut self) -> bool {
        if self.phase != SessionPhase::OpeningOutput {
            return false;
        }
        self.phase = SessionPhase::Running;
        true
    }

    pub fn stopping(&self) -> bool {
        matches!(self.phase, SessionPhase::Stopping { .. })
    }

    /// Returns true only for the first teardown. A newer disposition replaces
    /// an unsent idle command, and leaves an already dispatched one alone.
    pub fn stop(&mut self, mode: OutputRelease, end_request: bool) -> bool {
        match &mut self.phase {
            SessionPhase::Idle | SessionPhase::Faulted(_) => false,
            SessionPhase::Stopping {
                idle,
                end_request: end,
            } => {
                if idle.is_some() {
                    *idle = Some(mode);
                }
                *end |= end_request;
                false
            }
            _ => {
                self.phase = SessionPhase::Stopping {
                    idle: Some(mode),
                    end_request,
                };
                true
            }
        }
    }

    pub fn take_idle(&mut self) -> Option<OutputRelease> {
        match &mut self.phase {
            SessionPhase::Stopping { idle, .. } => idle.take(),
            _ => None,
        }
    }

    pub fn released(&mut self) -> bool {
        let SessionPhase::Stopping { end_request, .. } = self.phase else {
            return false;
        };
        self.phase = SessionPhase::Idle;
        end_request
    }

    pub fn fault(&mut self, phase: FaultPhase) {
        self.phase = SessionPhase::Faulted(phase);
    }
}

#[cfg(test)]
mod tests;
