//! JSON file encoding and error context; settings conversion belongs to the protocol.
use prysm_contract::{ControlError, application::Settings};
use prysm_protocol::settings::SettingsDocument;
use serde::{Serialize, de::DeserializeOwned};

pub(super) fn encode<C: Serialize, O: Serialize>(
    settings: Settings<C, O>,
) -> Result<Vec<u8>, ControlError> {
    let document = SettingsDocument::try_from(settings)?;
    serde_json::to_vec_pretty(&document).map_err(|error| ControlError::Adapter(Box::new(error)))
}

pub(super) fn decode<C: DeserializeOwned, O: DeserializeOwned>(
    bytes: &[u8],
) -> Result<Settings<C, O>, ControlError> {
    let document: SettingsDocument<C, O> =
        serde_json::from_slice(bytes).map_err(|error| ControlError::Invalid {
            field: "settings file",
            reason: error.to_string(),
        })?;
    document.try_into()
}
