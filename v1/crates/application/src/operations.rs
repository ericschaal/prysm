//! Bounded typed request/result history. Workflows own and complete their own IDs.
use prysm_contract::{
    ControlError, Failure,
    application::{ApplicationCommand, ApplicationEvent, OperationStatus},
    identity::{InstanceId, OperationId},
};
use std::collections::VecDeque;
const RETAINED: usize = 64;
struct Entry<S> {
    id: OperationId,
    command: ApplicationCommand<S>,
    status: OperationStatus,
}

pub(super) struct Operations<S> {
    instance: InstanceId,
    entries: VecDeque<Entry<S>>,
}
impl<S: PartialEq> Operations<S> {
    pub fn new(instance: InstanceId) -> Self {
        Self {
            instance,
            entries: VecDeque::new(),
        }
    }
    pub fn recognizes(
        &self,
        id: OperationId,
        command: &ApplicationCommand<S>,
    ) -> Result<bool, ControlError> {
        if id.instance != self.instance {
            return Err(ControlError::Conflict(
                "operation belongs to another application instance",
            ));
        }
        match self.entries.iter().find(|entry| entry.id == id) {
            Some(entry) if entry.command != *command => Err(ControlError::Conflict(
                "operation ID reused for a different command",
            )),
            entry => Ok(entry.is_some()),
        }
    }
    pub fn status(&self, id: OperationId) -> Result<OperationStatus, ControlError> {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.status.clone())
            .ok_or(ControlError::UnknownOperation)
    }
    pub fn accept(&mut self, id: OperationId, command: ApplicationCommand<S>) -> ApplicationEvent {
        if self.entries.len() == RETAINED {
            let completed = self
                .entries
                .iter()
                .position(|entry| !matches!(entry.status, OperationStatus::Pending))
                .expect("at most one replacement and one lighting operation are pending");
            self.entries.remove(completed);
        }
        self.entries.push_back(Entry {
            id,
            command,
            status: OperationStatus::Pending,
        });
        ApplicationEvent::OperationChanged {
            id,
            status: OperationStatus::Pending,
        }
    }
    pub fn finish(&mut self, id: OperationId, failure: Option<Failure>) -> ApplicationEvent {
        let status = failure.map_or(OperationStatus::Succeeded, OperationStatus::Failed);
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.id == id)
            .expect("pending operation retained");
        debug_assert!(matches!(entry.status, OperationStatus::Pending));
        entry.status = status.clone();
        ApplicationEvent::OperationChanged { id, status }
    }
}
