//! One configuration model for disk, the HTTP interface, and processor commands.
mod config;
mod identifiers;
mod processing;
mod resources;
mod state;

pub use config::{Config, UpdateConfig, WebConfig, WledConfig};
pub use identifiers::{ConfigRevision, OperationId};
pub use processing::{
    BrightnessPercent, ProcessingSettings, SampleDensity, ShadowLiftPercent, SmoothingSeconds,
    SpatialReachPercent,
};
pub use resources::{
    CaptureResolution, DeviceId, Input, LedCounts, Output, VideoPath, WledAddress,
};
pub use state::{
    ApiError, ErrorCode, Observation, OperationResponse, OperationStatus, PipelinePhase,
    PipelineStatus, Source, StateResponse, Target, WledStatus,
};

#[cfg(test)]
mod tests;
