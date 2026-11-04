//! One configuration model for disk, the HTTP interface, and processor commands.
mod calibration;
mod config;
pub use calibration::*;
mod identifiers;
mod preview;
mod processing;
mod resources;
mod settings;
mod state;

pub use config::Config;
pub use identifiers::{ConfigRevision, OperationId};
pub use preview::{PreviewColor, PreviewColors, PreviewFrame, PreviewMessage, PreviewViewport};
pub use processing::{
    BrightnessPercent, ColorAdjustment, ProcessingSettings, SampleDensity, ShadowLiftPercent,
    SmoothingSeconds, SpatialReachPercent,
};
pub use resources::{CaptureResolution, DeviceId, Input, LedCounts, Output, VideoPath};
pub use settings::{RequiredSetting, Settings, SettingsView, UpdateSettings};
pub use state::{
    ApiError, ErrorCode, Observation, OperationResponse, OperationStatus, PipelinePhase,
    PipelineStatus, Source, StateResponse, Target,
};

#[cfg(test)]
mod tests;
