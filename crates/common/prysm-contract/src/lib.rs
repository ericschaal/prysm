//! One configuration model for disk, the HTTP interface, and processor commands.
mod config;
mod identifiers;
mod preview;
mod processing;
mod resources;
mod state;

pub use config::{Config, UpdateConfig, WebConfig};
pub use identifiers::{ConfigRevision, OperationId};
pub use preview::{PreviewColor, PreviewColors, PreviewFrame, PreviewMessage, PreviewViewport};
pub use processing::{
    BrightnessPercent, ProcessingSettings, SampleDensity, ShadowLiftPercent, SmoothingSeconds,
    SpatialReachPercent,
};
pub use resources::{CaptureResolution, DeviceId, Input, LedCounts, Output, VideoPath};
pub use state::{
    ApiError, ErrorCode, Observation, OperationResponse, OperationStatus, PipelinePhase,
    PipelineStatus, Source, StateResponse, Target,
};

#[cfg(test)]
mod tests;
