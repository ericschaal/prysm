//! IDs distinguish process instances, operations and successive capture sessions.
use std::num::NonZeroU64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "wire", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "schema", schema(value_type = String, pattern = "^[1-9][0-9]*$"))]
pub struct InstanceId(
    #[cfg_attr(feature = "wire", serde(with = "crate::wire::nonzero_decimal"))] pub NonZeroU64,
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "wire", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct OperationId {
    pub instance: InstanceId,
    #[cfg_attr(feature = "wire", serde(with = "crate::wire::nonzero_decimal"))]
    #[cfg_attr(feature = "schema", schema(value_type = String, pattern = "^[1-9][0-9]*$"))]
    pub request: NonZeroU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId {
    pub instance: InstanceId,
    /// Coordinator-allocated; strictly increases within this application instance.
    pub sequence: NonZeroU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "wire", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "schema", schema(value_type = String, pattern = "^(0|[1-9][0-9]*)$"))]
pub struct SettingsRevision(
    #[cfg_attr(feature = "wire", serde(with = "crate::wire::decimal"))] pub u64,
);

/// Epoch zero is initial waiting. Capture advances it on discontinuities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StreamEpoch {
    pub session: SessionId,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameStamp {
    pub epoch: StreamEpoch,
    /// Strictly increases within an epoch; delayed or duplicate frames are stale.
    pub sequence: u64,
}
