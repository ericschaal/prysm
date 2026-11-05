//! Capture owns production, availability and confirmed native-resource cleanup.
use crate::{
    ControlError, Failure,
    identity::{FrameStamp, SessionId, StreamEpoch},
    observation::Observation,
};
use prysm_core::Frame;
use std::future::Future;

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "wire", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct CaptureCapabilities {
    /// Whether this source supports calibration using presented test patterns.
    pub supports_patterns: bool,
}

#[derive(Debug, Clone)]
pub struct CapturedFrame {
    pub stamp: FrameStamp,
    pub frame: Frame,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureState {
    Idle,
    Starting(SessionId),
    Available(StreamEpoch),
    Waiting {
        epoch: StreamEpoch,
        reason: String,
    },
    Stopping(SessionId),
    Failed {
        session: SessionId,
        failure: Failure,
        cleanup_complete: bool,
    },
}

pub struct CaptureStart<S> {
    pub session: SessionId,
    pub settings: S,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureStarted {
    pub session: SessionId,
}

/// A success value certifies release, including an open racing with Stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureStopped {
    pub session: SessionId,
}

pub trait CaptureControl: Clone + Send + Sync + 'static {
    type Settings: Clone + Send + Sync + 'static;
    fn capabilities(&self) -> CaptureCapabilities;
    fn validate(&self, settings: &Self::Settings) -> Result<(), ControlError>;
    fn start(
        &self,
        request: CaptureStart<Self::Settings>,
    ) -> impl Future<Output = Result<CaptureStarted, ControlError>> + Send;
    fn stop(
        &self,
        session: SessionId,
    ) -> impl Future<Output = Result<CaptureStopped, ControlError>> + Send;
    fn observe(
        &self,
    ) -> impl Future<Output = Result<Observation<CaptureState>, ControlError>> + Send;
}

/// One consuming endpoint; remains pending while capture is idle.
pub trait FrameSource: Send + 'static {
    /// None means owner closure, never temporary source unavailability.
    /// Implementations must be cancel-safe or retain partial reads internally.
    fn next_frame(
        &mut self,
    ) -> impl Future<Output = Result<Option<CapturedFrame>, ControlError>> + Send;
}
