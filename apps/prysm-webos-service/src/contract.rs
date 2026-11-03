//! The serialized Luna API, independent of native handles and process state.
use crate::config::Config;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Observation {
    pub return_value: bool,
    pub subscribed: bool,
    #[cfg_attr(feature = "schema", schema(required = true))]
    pub video_available: Option<bool>,
    pub revision: u64,
    pub subscribers: u32,
    pub pid: u32,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Capabilities {
    pub hardware_h264: bool,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct SupervisorStatus {
    pub config: Config,
    pub enabled: bool,
    pub receiver_connected: bool,
    pub capturing: bool,
    pub sender_running: bool,
    pub stopping: bool,
    #[cfg_attr(feature = "schema", schema(required = true))]
    pub power: Option<String>,
    #[cfg_attr(feature = "schema", schema(required = true))]
    pub error: Option<String>,
    #[cfg_attr(feature = "schema", schema(required = true))]
    pub last_log: Option<String>,
    pub capabilities: Capabilities,
}

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ServiceStatus {
    #[serde(flatten)]
    pub observation: Observation,
    #[serde(flatten)]
    pub supervisor: SupervisorStatus,
}

#[derive(Serialize)]
pub struct ProbeStatus {
    #[serde(flatten)]
    pub observation: Observation,
    pub power: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct ErrorReply {
    pub return_value: bool,
    pub error_text: String,
}

impl ErrorReply {
    #[must_use]
    pub fn new(error_text: String) -> Self {
        Self {
            return_value: false,
            error_text,
        }
    }
}
