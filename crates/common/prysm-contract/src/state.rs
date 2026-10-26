//! Lighting targets, observations, and HTTP operation results.
use crate::{ConfigRevision, OperationId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    #[default]
    Prysm,
    Wled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub power: bool,
    pub source: Source,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PipelineStatus {
    pub applied_revision: Option<ConfigRevision>,
    pub phase: PipelinePhase,
    pub frames_processed: u64,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PipelinePhase {
    Idle,
    Starting,
    Running,
    Failed,
    Completed,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WledStatus {
    pub power: bool,
    pub realtime: bool,
    pub realtime_source: Option<String>,
    pub live_override: u8,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Observation {
    pub pipeline_phase: PipelinePhase,
    pub power: bool,
    pub realtime: bool,
    pub realtime_source: Option<String>,
    pub live_override: u8,
}
impl Observation {
    #[must_use]
    pub fn matches(&self, target: Target) -> bool {
        if !target.power {
            !self.power
                && !self.realtime
                && self.live_override == 2
                && self.pipeline_phase == PipelinePhase::Idle
        } else if target.source == Source::Prysm {
            self.power
                && self.realtime
                && self.live_override == 0
                && self.pipeline_phase == PipelinePhase::Running
        } else {
            self.power
                && !self.realtime
                && self.live_override == 2
                && self.pipeline_phase == PipelinePhase::Idle
        }
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    Conflict,
    Persistence,
    Backend,
    Unavailable,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
}
#[derive(Debug, Default, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OperationStatus {
    #[default]
    Idle,
    Pending {
        id: OperationId,
    },
    Succeeded {
        id: OperationId,
    },
    Failed {
        id: OperationId,
        error: ApiError,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OperationResponse {
    pub operation_id: OperationId,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StateResponse {
    pub source: Source,
    pub observed: Option<Observation>,
    pub observation_error: Option<String>,
    pub operation: OperationStatus,
    pub wled_url: String,
    pub config_revision: ConfigRevision,
    pub applied_revision: Option<ConfigRevision>,
    pub pipeline: PipelineStatus,
    pub restart_required: bool,
}
