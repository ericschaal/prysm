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
    Starting { deadline: Instant },
    Running,
    StopApplication { deadline: Instant },
    StopFlow { deadline: Instant },
    StopOwners { deadline: Instant },
    CleaningUp { deadline: Instant },
    CleanupOverdue,
}

pub(super) struct Lifecycle {
    phase: Phase,
    failures: Vec<Failure>,
}
impl Lifecycle {
    pub fn new() -> Self {
        Self {
            phase: Phase::Starting {
                deadline: Instant::now() + DEADLINE,
            },
            failures: vec![],
        }
    }
    pub fn is_active(&self) -> bool {
        matches!(self.phase, Phase::Starting { .. } | Phase::Running)
    }
    pub fn deadline(&self) -> Option<Instant> {
        match self.phase {
            Phase::Starting { deadline }
            | Phase::StopApplication { deadline }
            | Phase::StopFlow { deadline }
            | Phase::StopOwners { deadline }
            | Phase::CleaningUp { deadline } => Some(deadline),
            Phase::Running | Phase::CleanupOverdue => None,
        }
    }
    fn is_cleaning_up(&self) -> bool {
        matches!(self.phase, Phase::CleaningUp { .. } | Phase::CleanupOverdue)
    }
    pub fn on_initialized(&mut self, instance: InstanceId, report: &mut impl FnMut(HostEvent)) {
        if !matches!(self.phase, Phase::Starting { .. }) {
            return;
        }
        self.phase = Phase::Running;
        report(HostEvent::Ready(instance));
    }
    pub fn request_stop(&mut self, owners: &mut Owners, report: &mut impl FnMut(HostEvent)) {
        if !self.is_active() {
            return;
        }
        report(HostEvent::Stopping);
        self.phase = Phase::StopApplication {
            deadline: Instant::now() + DEADLINE,
        };
        owners.application.request_stop();
    }
    pub fn on_timeout(&mut self, owners: &mut Owners, report: &mut impl FnMut(HostEvent)) {
        if self.deadline().is_none_or(|at| at > Instant::now()) {
            return;
        }
        if self.is_cleaning_up() {
            self.phase = Phase::CleanupOverdue;
            report(HostEvent::CleanupOverdue {
                pending: owners.pending(),
            });
        } else {
            let message = if matches!(self.phase, Phase::Starting { .. }) {
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
        if !self.is_cleaning_up() {
            self.phase = Phase::CleaningUp {
                deadline: Instant::now() + DEADLINE,
            };
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
        let error = match completion.result {
            Ok(Ok(_)) if completion.expected => None,
            Ok(Ok(_)) => Some(ControlError::Unavailable("owner exited unexpectedly")),
            Ok(Err(error)) => Some(error),
            Err(error)
                if owner == Owner::Application && self.is_cleaning_up() && error.is_cancelled() =>
            {
                None
            }
            Err(error) => Some(ControlError::RecoveryRequired(Box::new(error))),
        };
        if let Some(error) = error {
            self.on_failure(owner.name(), error, owners, report);
        } else {
            match (owner, self.phase) {
                (Owner::Application, Phase::StopApplication { deadline }) => {
                    self.phase = Phase::StopFlow { deadline };
                    owners.stop_settings();
                    owners.flow.request_stop();
                }
                (Owner::Processing, Phase::StopFlow { deadline }) => {
                    self.phase = Phase::StopOwners { deadline };
                    owners.capture.request_stop();
                    owners.output.request_stop();
                }
                _ => {}
            }
        }
    }
}
