//! Source selection belongs to the server host, not either capture adapter.
use prysm_capture_device::DeviceCaptureSettings;
use prysm_remote_capture::RemoteCaptureSettings;

#[cfg(any(feature = "headless", feature = "tui", test))]
mod host;
#[cfg(any(feature = "headless", feature = "tui", test))]
mod lifecycle;
#[cfg(any(feature = "headless", feature = "tui", test))]
mod owners;
#[cfg(any(feature = "headless", feature = "tui", test))]
pub use host::{HostError, HostEvent};
pub use prysm_output_terminal::TerminalSettings;

use prysm_contract::application::Settings;
use prysm_output_wled::WledSettings;

#[cfg(any(feature = "headless", feature = "tui"))]
mod runtime;
#[cfg(feature = "headless")]
pub use runtime::run_headless;
#[cfg(feature = "tui")]
pub use runtime::run_tui;

pub type HeadlessSettings = Settings<DeviceCaptureSettings, WledSettings>;

#[cfg(test)]
struct FixedSettings<S>(S);
#[cfg(test)]
impl<S: Clone + Send + Sync + 'static> prysm_contract::storage::SettingsRepository<S>
    for FixedSettings<S>
{
    async fn load(&self) -> Result<Option<S>, prysm_contract::ControlError> {
        Ok(Some(self.0.clone()))
    }
    async fn save(&self, _: S) -> Result<(), prysm_contract::ControlError> {
        Err(prysm_contract::ControlError::Unavailable(
            "headless settings are read-only",
        ))
    }
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "wire", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "wire",
    serde(tag = "adapter", content = "settings", rename_all = "snake_case")
)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub enum ServerCaptureSettings {
    Device(DeviceCaptureSettings),
    Remote(RemoteCaptureSettings),
}

#[cfg(feature = "schema")]
pub fn document() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(components(schemas(ServerCaptureSettings)))]
    struct Contract;
    <Contract as utoipa::OpenApi>::openapi()
}

#[cfg(any(feature = "headless", feature = "tui"))]
fn new_instance() -> prysm_contract::identity::InstanceId {
    use prysm_contract::identity::InstanceId;
    use std::num::NonZeroU64;
    // RandomState is independently seeded by std; IDs are lineage, not credentials.
    {
        use std::hash::{BuildHasher, Hasher};
        let value = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish();
        InstanceId(NonZeroU64::new(value).unwrap_or(NonZeroU64::MIN))
    }
}
