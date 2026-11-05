//! One owner serializes device modes and color writes, including delayed writes.
use crate::{
    ControlError, Failure,
    identity::{FrameStamp, SessionId, SettingsRevision, StreamEpoch},
    observation::Observation,
};
use prysm_core::EdgeColors;
use std::future::Future;

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "wire", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct OutputCapabilities {
    pub native_mode: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleOutputMode {
    Off,
    Native,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputApplied {
    /// Locally applied/sent; the device has not acknowledged actual display.
    Sent,
    Confirmed,
}

#[derive(Debug, Clone)]
pub struct ColorFrame {
    pub stamp: FrameStamp,
    pub revision: SettingsRevision,
    pub colors: EdgeColors,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferResult {
    Queued,
    DiscardedStale,
}

#[derive(Debug, Clone)]
pub struct OutputState {
    pub admitted: Option<StreamEpoch>,
    pub held: bool,
    pub idle_mode: Option<IdleOutputMode>,
    pub confirmation: Option<OutputApplied>,
    pub failure: Option<Failure>,
}

pub trait OutputControl: Clone + Send + Sync + 'static {
    type Settings: Clone + Send + Sync + 'static;
    fn capabilities(&self) -> OutputCapabilities;
    fn validate(&self, settings: &Self::Settings) -> Result<(), ControlError>;
    fn configure(
        &self,
        settings: Self::Settings,
    ) -> impl Future<Output = Result<OutputApplied, ControlError>> + Send;
    fn admit(
        &self,
        epoch: StreamEpoch,
    ) -> impl Future<Output = Result<OutputApplied, ControlError>> + Send;
    /// Invalidate colors at/before this epoch. Retain at most one later candidate.
    fn hold(
        &self,
        epoch: StreamEpoch,
        colors: EdgeColors,
    ) -> impl Future<Output = Result<OutputApplied, ControlError>> + Send;
    /// Close admission and account for outstanding writes before the idle mode.
    fn finish(
        &self,
        session: SessionId,
        mode: IdleOutputMode,
    ) -> impl Future<Output = Result<OutputApplied, ControlError>> + Send;
    fn observe(
        &self,
    ) -> impl Future<Output = Result<Observation<OutputState>, ControlError>> + Send;
}

pub trait ColorSink: Send + 'static {
    /// Nonblocking bounded offer, never proof of delivery or display.
    fn offer(&self, frame: ColorFrame) -> Result<OfferResult, ControlError>;
}
