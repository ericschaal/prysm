//! capture-desktop contract skeleton; device behavior remains deferred.
use prysm_core::CaptureResolution;
#[cfg(feature = "schema")]
use prysm_protocol::schema::resolution as resolution_schema;
#[cfg(feature = "wire")]
use prysm_protocol::wire::resolution as resolution_wire;
use std::{num::NonZeroU32, path::PathBuf};

/// These choices belong to the desktop adapter, never the shared application.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "wire", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "wire", serde(tag = "source", rename_all = "snake_case"))]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum DesktopCaptureSettings {
    Device {
        #[cfg_attr(feature = "schema", schema(value_type = String))]
        path: PathBuf,
        #[cfg_attr(feature = "wire", serde(with = "resolution_wire"))]
        #[cfg_attr(feature = "schema", schema(schema_with = resolution_schema))]
        resolution: CaptureResolution,
        #[cfg_attr(feature = "schema", schema(value_type = u32, minimum = 1, maximum = 4294967295_u64))]
        rate: NonZeroU32,
    },
    File {
        #[cfg_attr(feature = "schema", schema(value_type = String))]
        path: PathBuf,
        #[cfg_attr(feature = "wire", serde(with = "resolution_wire"))]
        #[cfg_attr(feature = "schema", schema(schema_with = resolution_schema))]
        resolution: CaptureResolution,
        #[cfg_attr(feature = "schema", schema(value_type = u32, minimum = 1, maximum = 4294967295_u64))]
        rate: NonZeroU32,
    },
}

#[cfg(feature = "schema")]
pub mod schema;
