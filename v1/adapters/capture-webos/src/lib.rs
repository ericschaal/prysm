//! capture-webos contract skeleton; device behavior remains deferred.
use prysm_core::CaptureResolution;
#[cfg(feature = "schema")]
use prysm_protocol::schema::resolution as resolution_schema;
#[cfg(feature = "wire")]
use prysm_protocol::wire::resolution as resolution_wire;
use std::num::NonZeroU32;

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "wire", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "wire", serde(deny_unknown_fields))]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct TvCaptureSettings {
    #[cfg_attr(feature = "wire", serde(with = "resolution_wire"))]
    #[cfg_attr(feature = "schema", schema(schema_with = resolution_schema))]
    pub resolution: CaptureResolution,
    #[cfg_attr(feature = "schema", schema(value_type = u32, minimum = 1, maximum = 4294967295_u64))]
    pub rate: NonZeroU32,
}

#[cfg(feature = "schema")]
pub mod schema;
