//! Snapshot and subscription must be established at the same serialized point.
use crate::{ControlError, identity::InstanceId};
use futures::stream::BoxStream;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub instance: InstanceId,
    pub sequence: u64,
}

#[derive(Debug, Clone)]
pub struct Versioned<S> {
    pub cursor: Cursor,
    pub state: S,
}

#[derive(Debug, Clone)]
pub enum Update<S, E> {
    Snapshot(Versioned<S>),
    Event {
        cursor: Cursor,
        event: E,
    },
    /// Discard the old subscription and acquire a new snapshot/subscription.
    ResyncRequired,
}

pub struct Observation<S, E = std::convert::Infallible> {
    pub initial: Versioned<S>,
    /// Bounded live updates. Closure means the observer/owner connection ended.
    pub updates: BoxStream<'static, Result<Update<S, E>, ControlError>>,
}
