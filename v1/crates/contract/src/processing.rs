//! Configuration and completion barriers; deliberately no start/stop operations.
use crate::{
    ControlError, Failure,
    identity::{SessionId, SettingsRevision},
    observation::Observation,
};
use prysm_core::Config;
use std::future::Future;

#[derive(Debug, Clone, Copy)]
pub struct ProcessingUpdate {
    pub revision: SettingsRevision,
    pub config: Config,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Configured {
    pub revision: SettingsRevision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quiescent {
    pub session: SessionId,
}

#[derive(Debug, Clone)]
pub struct ProcessingState {
    pub configured_revision: Option<SettingsRevision>,
    /// Last frame processed with these settings; does not confirm output delivery.
    pub applied_revision: Option<SettingsRevision>,
    pub failure: Option<Failure>,
}

pub trait ProcessingControl: Clone + Send + Sync + 'static {
    /// Success stages settings at a frame boundary; it does not assert a frame ran.
    fn configure(
        &self,
        update: ProcessingUpdate,
    ) -> impl Future<Output = Result<Configured, ControlError>> + Send;
    /// After capture cleanup, seal and drain/discard work for the old session.
    fn barrier(
        &self,
        session: SessionId,
    ) -> impl Future<Output = Result<Quiescent, ControlError>> + Send;
    fn observe(
        &self,
    ) -> impl Future<Output = Result<Observation<ProcessingState>, ControlError>> + Send;
}
