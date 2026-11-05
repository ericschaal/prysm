//! Bounded acceptance/result history. At most one accepted operation is pending.
use prysm_contract::{
    ControlError, Failure,
    application::{ApplicationEvent, LightingTarget, OperationStatus},
    identity::{InstanceId, OperationId},
};
use std::collections::VecDeque;
const RETAINED: usize = 64;
struct Entry {
    id: OperationId,
    target: LightingTarget,
    status: OperationStatus,
}

pub(super) struct Operations {
    instance: InstanceId,
    entries: VecDeque<Entry>,
    active: Option<OperationId>,
}
impl Operations {
    pub fn new(instance: InstanceId) -> Self {
        Self {
            instance,
            entries: VecDeque::new(),
            active: None,
        }
    }
    pub fn recognizes(
        &self,
        id: OperationId,
        target: LightingTarget,
    ) -> Result<bool, ControlError> {
        if id.instance != self.instance {
            return Err(ControlError::Conflict(
                "operation belongs to another application instance",
            ));
        }
        match self.entries.iter().find(|entry| entry.id == id) {
            Some(entry) if entry.target != target => Err(ControlError::Conflict(
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
    /// Acceptance follows validation; it supersedes the prior pending operation.
    pub fn accept(&mut self, id: OperationId, target: LightingTarget) -> Vec<ApplicationEvent> {
        let mut events = Vec::with_capacity(2);
        if let Some(event) = self.finish(Some(Failure {
            message: "superseded by stop".into(),
        })) {
            events.push(event);
        }
        // finish() discharged the only pending entry, so eviction cannot lose work.
        if self.entries.len() == RETAINED {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry {
            id,
            target,
            status: OperationStatus::Pending,
        });
        self.active = Some(id);
        events.push(ApplicationEvent::OperationChanged {
            id,
            status: OperationStatus::Pending,
        });
        events
    }
    pub fn finish(&mut self, failure: Option<Failure>) -> Option<ApplicationEvent> {
        let id = self.active.take()?;
        let status = failure.map_or(OperationStatus::Succeeded, OperationStatus::Failed);
        self.entries
            .iter_mut()
            .find(|entry| entry.id == id)
            .expect("pending operation retained")
            .status = status.clone();
        Some(ApplicationEvent::OperationChanged { id, status })
    }
}
