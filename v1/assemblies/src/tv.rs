//! Platform-only UI surface. Application lighting/calibration stay on ApplicationControl.
use prysm_contract::{ControlError, OwnerStopped, application::ApplicationControl};
use std::future::Future;

/// Embedded host's local application endpoint. The portal receives a clone of this same port.
pub struct LunaApplicationBridge<A> {
    pub application: A,
}

impl<A: ApplicationControl> LunaApplicationBridge<A> {
    pub fn new(application: A) -> Self {
        Self { application }
    }

    pub async fn run(
        self,
        _shutdown: impl Future<Output = ()> + Send,
    ) -> Result<OwnerStopped, ControlError> {
        Err(ControlError::Unavailable(
            "skeleton: Luna application routing is not implemented",
        ))
    }
}

/// Always present in TV hosts, including capture-only hosts with no local application.
pub struct LunaPlatformBridge;

impl LunaPlatformBridge {
    pub async fn status(&self) -> Result<TvHostStatus, ControlError> {
        Err(ControlError::Unavailable(
            "skeleton: webOS platform status is not implemented",
        ))
    }
    pub async fn command(&self, _command: TvHostCommand) -> Result<(), ControlError> {
        Err(ControlError::Unavailable(
            "skeleton: webOS platform commands are not implemented",
        ))
    }
}
#[derive(Clone)]
#[cfg_attr(feature = "wire", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct TvHostStatus {
    pub service_ready: bool,
    pub capture_available: bool,
    pub forwarding: bool,
}

#[derive(Clone)]
#[cfg_attr(feature = "wire", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "wire", serde(rename_all = "snake_case"))]
pub enum TvHostCommand {
    InspectService,
    RepairService,
}

#[cfg(feature = "schema")]
pub fn document() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(components(schemas(TvHostStatus, TvHostCommand)))]
    struct Contract;
    <Contract as utoipa::OpenApi>::openapi()
}
