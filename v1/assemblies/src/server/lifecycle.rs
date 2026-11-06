//! Host phase transitions; no session identities or device operations.
use super::{
    host::{HostError, HostEvent},
    owners::{Completion, Owner, Owners},
};
use prysm_contract::{ControlError, Failure, OwnerStopped};
use std::time::Duration;
use tokio::time::Instant;

pub(super) const DEADLINE: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Starting,
    Running,
    StopApplication,
    StopFlow,
    StopOwners,
    Terminal,
}

pub(super) struct Lifecycle {
    phase: Phase,
    deadline: Option<Instant>,
    failures: Vec<Failure>,
}
impl Lifecycle {
    pub fn new() -> Self {
        Self {
            phase: Phase::Starting,
            deadline: Some(Instant::now() + DEADLINE),
            failures: vec![],
        }
    }
    pub fn accepting(&self) -> bool {
        self.phase == Phase::Running
    }
    pub fn observing(&self) -> bool {
        matches!(self.phase, Phase::Starting | Phase::Running)
    }
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
    pub fn initialized(&mut self) -> bool {
        if self.phase != Phase::Starting {
            return false;
        }
        self.phase = Phase::Running;
        self.deadline = None;
        true
    }
    pub fn stop(&mut self, owners: &mut Owners) {
        self.phase = Phase::StopApplication;
        self.deadline = Some(Instant::now() + DEADLINE);
        owners.application.request_stop();
    }
    pub fn timed_out(&mut self, owners: &mut Owners, report: &mut impl FnMut(HostEvent)) {
        if self.phase == Phase::Terminal {
            self.deadline = None;
            report(HostEvent::CleanupOverdue {
                pending: owners.pending(),
            });
        } else {
            let message = if self.phase == Phase::Starting {
                "initialization deadline expired"
            } else {
                "coordinated shutdown deadline expired"
            };
            self.failure("host", ControlError::Unavailable(message), report);
            self.terminate(owners);
        }
    }
    pub fn result(self) -> Result<OwnerStopped, HostError> {
        if self.failures.is_empty() {
            Ok(OwnerStopped)
        } else {
            Err(HostError {
                failures: self.failures,
            })
        }
    }
    pub fn failure(
        &mut self,
        context: &str,
        error: impl Into<Failure>,
        report: &mut impl FnMut(HostEvent),
    ) {
        let mut failure = error.into();
        failure.message = format!("{context}: {}", failure.message);
        report(HostEvent::Failure(failure.clone()));
        self.failures.push(failure);
    }
    pub fn terminate(&mut self, owners: &mut Owners) {
        if self.phase != Phase::Terminal {
            self.phase = Phase::Terminal;
            self.deadline = Some(Instant::now() + DEADLINE);
            owners.terminate();
        }
    }
    pub fn exited(
        &mut self,
        owner: Owner,
        completion: Completion,
        owners: &mut Owners,
        report: &mut impl FnMut(HostEvent),
    ) {
        let task = owners.task(owner);
        let expected = task.take_completed();
        let error = match completion {
            Ok(Ok(_)) if expected => None,
            Ok(Ok(_)) => Some(ControlError::Unavailable("owner exited unexpectedly")),
            Ok(Err(error)) => Some(error),
            Err(error)
                if owner == Owner::Application
                    && self.phase == Phase::Terminal
                    && error.is_cancelled() =>
            {
                None
            }
            Err(error) => Some(ControlError::RecoveryRequired(Box::new(error))),
        };
        if let Some(error) = error {
            self.failure(owner.name(), error, report);
            self.terminate(owners);
        } else if owner == Owner::Application && self.phase == Phase::StopApplication {
            self.phase = Phase::StopFlow;
            owners.stop_settings();
            owners.flow.request_stop();
        } else if owner == Owner::Processing && self.phase == Phase::StopFlow {
            self.phase = Phase::StopOwners;
            owners.capture.request_stop();
            owners.output.request_stop();
        }
    }
}
