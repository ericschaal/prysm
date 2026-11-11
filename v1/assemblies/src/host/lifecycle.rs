//! Host phase transitions, deadlines, and lifecycle notifications.
use super::{
    HostError, HostEvent,
    owners::{Completion, Owner, Owners},
};
use prysm_contract::{ControlError, Failure, OwnerStopped, identity::InstanceId};
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
    CleaningUp,
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
    pub fn is_active(&self) -> bool {
        matches!(self.phase, Phase::Starting | Phase::Running)
    }
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
    pub fn on_initialized(&mut self, instance: InstanceId, report: &mut impl FnMut(HostEvent)) {
        if self.phase != Phase::Starting {
            return;
        }
        self.phase = Phase::Running;
        self.deadline = None;
        report(HostEvent::Ready(instance));
    }
    pub fn request_stop(&mut self, owners: &mut Owners, report: &mut impl FnMut(HostEvent)) {
        if !self.is_active() {
            return;
        }
        report(HostEvent::Stopping);
        self.phase = Phase::StopApplication;
        self.deadline = Some(Instant::now() + DEADLINE);
        owners.application.request_stop();
    }
    pub fn on_timeout(&mut self, owners: &mut Owners, report: &mut impl FnMut(HostEvent)) {
        if self.deadline.is_none_or(|at| at > Instant::now()) {
            return;
        }
        if self.phase == Phase::CleaningUp {
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
            self.on_failure("host", ControlError::Unavailable(message), owners, report);
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
    pub fn on_failure(
        &mut self,
        context: &str,
        error: impl Into<Failure>,
        owners: &mut Owners,
        report: &mut impl FnMut(HostEvent),
    ) {
        let mut failure = error.into();
        failure.message = format!("{context}: {}", failure.message);
        report(HostEvent::Failure(failure.clone()));
        self.failures.push(failure);
        if self.phase != Phase::CleaningUp {
            self.phase = Phase::CleaningUp;
            self.deadline = Some(Instant::now() + DEADLINE);
            owners.terminate();
        }
    }
    pub fn on_owner_exited(
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
                    && self.phase == Phase::CleaningUp
                    && error.is_cancelled() =>
            {
                None
            }
            Err(error) => Some(ControlError::RecoveryRequired(Box::new(error))),
        };
        if let Some(error) = error {
            self.on_failure(owner.name(), error, owners, report);
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
