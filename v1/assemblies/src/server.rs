//! Source selection belongs to the server host, not either capture adapter.
use prysm_capture_desktop::DesktopCaptureSettings;
use prysm_remote_capture::RemoteCaptureSettings;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "wire", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "wire",
    serde(tag = "adapter", content = "settings", rename_all = "snake_case")
)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum ServerCaptureSettings {
    Desktop(DesktopCaptureSettings),
    Remote(RemoteCaptureSettings),
}

#[cfg(feature = "schema")]
pub fn document() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(components(schemas(ServerCaptureSettings)))]
    struct Contract;
    <Contract as utoipa::OpenApi>::openapi()
}
