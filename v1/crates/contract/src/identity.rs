//! IDs distinguish process instances, operations and successive capture sessions.
use std::num::NonZeroU64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InstanceId(pub NonZeroU64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OperationId {
    pub instance: InstanceId,
    pub request: NonZeroU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId {
    pub instance: InstanceId,
    /// Coordinator-allocated; strictly increases within this application instance.
    pub sequence: NonZeroU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SettingsRevision(pub u64);

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
