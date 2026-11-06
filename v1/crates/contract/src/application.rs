//! The application owns intent, settings, calibration and accepted operations.
use crate::{
    ControlError, Failure,
    calibration::{
        CalibrationCommand, CalibrationProfile, CalibrationReply, CalibrationStatus, ProfileId,
    },
    capture::{CaptureCapabilities, CaptureState},
    identity::{InstanceId, OperationId, SessionId, SettingsRevision},
    observation::Observation,
    output::{OutputCapabilities, OutputState},
};
use prysm_core::Config;
use std::future::Future;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightingTarget {
    Off,
    Processed,
    DeviceNative,
}

/// C/O are adapter-owned types. The shared application never matches vendor enums.
#[derive(Debug, Clone)]
pub struct Settings<C, O> {
    pub capture: Option<C>,
    pub output: Option<O>,
    pub processing: Config,
    pub profiles: Vec<CalibrationProfile>,
    pub active_profile: Option<ProfileId>,
}

impl<C, O> Settings<C, O> {
    /// Validate aggregate relationships, including an intentionally unconfigured setup.
    /// Adapter-specific settings are validated by their owning ports.
    pub fn validate(&self) -> Result<(), ControlError> {
        if let Some(id) = self.active_profile
            && !self.profiles.iter().any(|profile| profile.id == id)
        {
            return Err(ControlError::Invalid {
                field: "active_profile",
                reason: "profile does not exist".into(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum ApplicationCommand<S> {
    SetTarget(LightingTarget),
    ReplaceSettings {
        expected: SettingsRevision,
        settings: S,
    },
}

#[derive(Debug, Clone)]
pub struct Request<T> {
    /// Scoped to the expected process instance; also correlates an uncertain reply.
    pub id: OperationId,
    pub command: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accepted {
    pub operation: OperationId,
}

#[derive(Debug, Clone)]
pub enum OperationStatus {
    Pending,
    Succeeded,
    Failed(Failure),
}

#[derive(Debug, Clone)]
pub enum SessionState {
    Idle,
    Starting(SessionId),
    Running(SessionId),
    Waiting(SessionId),
    Stopping(SessionId),
    Failed {
        session: SessionId,
        failure: Failure,
        cleanup_complete: bool,
    },
}

#[derive(Debug, Clone)]
pub struct ApplicationSnapshot<S> {
    pub instance: InstanceId,
    pub settings: S,
    pub saved_revision: SettingsRevision,
    pub configured_revision: Option<SettingsRevision>,
    pub applied_revision: Option<SettingsRevision>,
    pub desired: LightingTarget,
    pub session: SessionState,
    pub capture: CaptureCapabilities,
    pub output: OutputCapabilities,
    pub observed_capture: Option<CaptureState>,
    pub observed_output: Option<OutputState>,
    pub calibration: CalibrationStatus,
}

#[derive(Debug, Clone)]
pub enum ApplicationEvent {
    OperationChanged {
        id: OperationId,
        status: OperationStatus,
    },
}

pub type ApplicationObservation<S> = Observation<ApplicationSnapshot<S>, ApplicationEvent>;

pub trait ApplicationControl: Clone + Send + Sync + 'static {
    type Settings: Clone + Send + Sync + 'static;
    /// Enqueue/accept separately from completion; accepted work survives caller loss.
    fn submit(
        &self,
        request: Request<ApplicationCommand<Self::Settings>>,
    ) -> impl Future<Output = Result<Accepted, ControlError>> + Send;
    fn observe(
        &self,
    ) -> impl Future<Output = Result<ApplicationObservation<Self::Settings>, ControlError>> + Send;
    fn operation(
        &self,
        id: OperationId,
    ) -> impl Future<Output = Result<OperationStatus, ControlError>> + Send;
    /// Same serialized coordinator, with a private typed reply for pairing credentials.
    fn calibrate(
        &self,
        request: Request<CalibrationCommand>,
    ) -> impl Future<Output = Result<CalibrationReply, ControlError>> + Send;
}
