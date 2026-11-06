//! UI-facing projection and adapter requests. Mapping to ApplicationControl is deferred.
//! Configure requests must preserve settings outside this projection when mapped.
use crate::application::{Cursor, LightingTarget, OperationId, SettingsRevision};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ClientSnapshot<C = Value, O = Value> {
    pub cursor: Cursor,
    pub revision: SettingsRevision,
    #[cfg_attr(feature = "schema", schema(required = true))]
    pub capture: Option<C>,
    #[cfg_attr(feature = "schema", schema(required = true))]
    pub output: Option<O>,
    pub target: LightingTarget,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum ClientUpdate<C = Value, O = Value> {
    Snapshot { snapshot: ClientSnapshot<C, O> },
    ResyncRequired,
}

#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ConfigureRequest<S = Value> {
    pub id: OperationId,
    pub expected: SettingsRevision,
    pub settings: S,
}

#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct TargetRequest {
    pub id: OperationId,
    pub target: LightingTarget,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum ClientFailure {
    Unavailable { message: String },
    Rejected { message: String },
    OutcomeUnknown { operation: OperationId },
    UnknownOperation,
}

/// These private presentation messages never appear in ClientSnapshot.
#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct CalibrationPresentation {
    pub session: String,
    pub pairing: String,
    pub pattern: PatternRequest,
}

#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct PatternRequest {
    pub id: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum PlaybackFeedback {
    Ready,
    Failed { message: String },
    Released,
}

#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct PresentationFeedback {
    pub id: OperationId,
    pub session: String,
    pub playback: PlaybackFeedback,
}

#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct EndPresentationRequest {
    pub id: OperationId,
    pub session: String,
}
