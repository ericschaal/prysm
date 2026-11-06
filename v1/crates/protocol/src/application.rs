//! Application wire values. Conversions keep JSON and schema choices outside ports.
use prysm_contract as domain;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "schema", schema(value_type = String, pattern = "^[1-9][0-9]*$"))]
pub struct InstanceId(#[serde(with = "crate::wire::nonzero_decimal")] pub NonZeroU64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct OperationId {
    pub instance: InstanceId,
    #[serde(with = "crate::wire::nonzero_decimal")]
    #[cfg_attr(feature = "schema", schema(value_type = String, pattern = "^[1-9][0-9]*$"))]
    pub request: NonZeroU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "schema", schema(value_type = String, pattern = "^(0|[1-9][0-9]*)$"))]
pub struct SettingsRevision(#[serde(with = "crate::wire::decimal")] pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Cursor {
    pub instance: InstanceId,
    #[serde(with = "crate::wire::decimal")]
    #[cfg_attr(feature = "schema", schema(value_type = String, pattern = "^(0|[1-9][0-9]*)$"))]
    pub sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum LightingTarget {
    Off,
    Processed,
    DeviceNative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Accepted {
    pub operation: OperationId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct CaptureCapabilities {
    /// Whether this source supports calibration using presented test patterns.
    pub supports_patterns: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct OutputCapabilities {
    pub native_mode: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum FailureKind {
    Unavailable,
    Invalid,
    Conflict,
    OutcomeUnknown,
    CleanupPending,
    UnknownOperation,
    Adapter,
    RecoveryRequired,
    Interrupted,
}

/// Local source errors and their implementation types never enter the wire format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Failure {
    pub kind: FailureKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "failure", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum OperationStatus {
    Pending,
    Succeeded,
    Failed(Failure),
}

impl From<domain::identity::InstanceId> for InstanceId {
    fn from(value: domain::identity::InstanceId) -> Self {
        Self(value.0)
    }
}
impl From<InstanceId> for domain::identity::InstanceId {
    fn from(value: InstanceId) -> Self {
        Self(value.0)
    }
}
impl From<domain::identity::OperationId> for OperationId {
    fn from(value: domain::identity::OperationId) -> Self {
        Self {
            instance: value.instance.into(),
            request: value.request,
        }
    }
}
impl From<OperationId> for domain::identity::OperationId {
    fn from(value: OperationId) -> Self {
        Self {
            instance: value.instance.into(),
            request: value.request,
        }
    }
}
impl From<domain::identity::SettingsRevision> for SettingsRevision {
    fn from(value: domain::identity::SettingsRevision) -> Self {
        Self(value.0)
    }
}
impl From<SettingsRevision> for domain::identity::SettingsRevision {
    fn from(value: SettingsRevision) -> Self {
        Self(value.0)
    }
}
impl From<domain::observation::Cursor> for Cursor {
    fn from(value: domain::observation::Cursor) -> Self {
        Self {
            instance: value.instance.into(),
            sequence: value.sequence,
        }
    }
}
impl From<domain::application::LightingTarget> for LightingTarget {
    fn from(value: domain::application::LightingTarget) -> Self {
        match value {
            domain::application::LightingTarget::Off => Self::Off,
            domain::application::LightingTarget::Processed => Self::Processed,
            domain::application::LightingTarget::DeviceNative => Self::DeviceNative,
        }
    }
}
impl From<LightingTarget> for domain::application::LightingTarget {
    fn from(value: LightingTarget) -> Self {
        match value {
            LightingTarget::Off => Self::Off,
            LightingTarget::Processed => Self::Processed,
            LightingTarget::DeviceNative => Self::DeviceNative,
        }
    }
}
impl From<domain::application::Accepted> for Accepted {
    fn from(value: domain::application::Accepted) -> Self {
        Self {
            operation: value.operation.into(),
        }
    }
}
impl From<domain::capture::CaptureCapabilities> for CaptureCapabilities {
    fn from(value: domain::capture::CaptureCapabilities) -> Self {
        Self {
            supports_patterns: value.supports_patterns,
        }
    }
}
impl From<domain::output::OutputCapabilities> for OutputCapabilities {
    fn from(value: domain::output::OutputCapabilities) -> Self {
        Self {
            native_mode: value.native_mode,
        }
    }
}
impl From<domain::FailureKind> for FailureKind {
    fn from(value: domain::FailureKind) -> Self {
        match value {
            domain::FailureKind::Unavailable => Self::Unavailable,
            domain::FailureKind::Invalid => Self::Invalid,
            domain::FailureKind::Conflict => Self::Conflict,
            domain::FailureKind::OutcomeUnknown => Self::OutcomeUnknown,
            domain::FailureKind::CleanupPending => Self::CleanupPending,
            domain::FailureKind::UnknownOperation => Self::UnknownOperation,
            domain::FailureKind::Adapter => Self::Adapter,
            domain::FailureKind::RecoveryRequired => Self::RecoveryRequired,
            domain::FailureKind::Interrupted => Self::Interrupted,
        }
    }
}
impl From<domain::Failure> for Failure {
    fn from(value: domain::Failure) -> Self {
        Self {
            kind: value.kind.into(),
            message: value.message,
        }
    }
}
impl From<domain::application::OperationStatus> for OperationStatus {
    fn from(value: domain::application::OperationStatus) -> Self {
        match value {
            domain::application::OperationStatus::Pending => Self::Pending,
            domain::application::OperationStatus::Succeeded => Self::Succeeded,
            domain::application::OperationStatus::Failed(failure) => Self::Failed(failure.into()),
        }
    }
}
