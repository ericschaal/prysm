use prysm_core::EdgeColors;
use prysm_ports::{IdleMode, LedOutput, OutputFactory, Result, ValidationError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Settings of [`NullOutput`]: it takes none, so only `{ "kind": "null" }` is valid.
///
/// Decoded as a map so that `validate` can name a stray field.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NullSettings(BTreeMap<String, ()>);

impl NullSettings {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// An object without fields.
#[cfg(feature = "schema")]
impl utoipa::PartialSchema for NullSettings {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::schema::{AdditionalProperties, ObjectBuilder};
        ObjectBuilder::new()
            .description(Some("The null output takes no settings."))
            .additional_properties(Some(AdditionalProperties::FreeForm(false)))
            .into()
    }
}

#[cfg(feature = "schema")]
impl utoipa::ToSchema for NullSettings {}

/// An output that drives no LEDs: writes and idles succeed and do nothing.
///
/// For hosts whose surfaces (such as the terminal preview) show the colors, and
/// for running the pipeline without hardware. Registers kind `"null"`.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullOutput;

impl OutputFactory for NullOutput {
    const KIND: &'static str = "null";
    type Settings = NullSettings;

    fn validate(&self, settings: &NullSettings) -> Result<(), ValidationError> {
        match settings.0.keys().next() {
            None => Ok(()),
            Some(field) => Err(ValidationError::new(
                field.as_str(),
                "the null output takes no settings",
            )),
        }
    }

    fn open(&self, _settings: &NullSettings) -> Result<Box<dyn LedOutput>> {
        Ok(Box::new(NullLeds))
    }
}

struct NullLeds;

impl LedOutput for NullLeds {
    fn write(&mut self, _colors: &EdgeColors) -> Result<()> {
        Ok(())
    }

    fn idle(self: Box<Self>, _mode: IdleMode) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_ports::{AdapterSettings, Registry, Settings};

    #[test]
    fn null_output_accepts_writes_and_idles_and_rejects_fields() {
        let mut registry = Registry::default();
        registry.add_output(NullOutput);
        let null = AdapterSettings::new(NullOutput::KIND, &NullSettings::new());
        let settings = Settings {
            output: Some(null.clone()),
            ..Settings::default()
        };
        registry.validate(&settings).unwrap();

        let mut output = registry.open_output(&null).unwrap();
        output.write(&EdgeColors::default()).unwrap();
        output.idle(IdleMode::Native).unwrap();

        let extra = NullSettings(BTreeMap::from([("host".into(), ())]));
        let error = NullOutput.validate(&extra).unwrap_err();
        assert_eq!(error.field, "host");
        let extra = AdapterSettings::new(NullOutput::KIND, &extra);
        assert!(registry.open_output(&extra).is_err());
    }

    #[cfg(feature = "schema")]
    #[test]
    fn schema_is_an_object_without_fields() {
        use utoipa::{PartialSchema, ToSchema};
        let schema = NullSettings::schema();
        let utoipa::openapi::RefOr::T(utoipa::openapi::schema::Schema::Object(object)) = schema
        else {
            panic!("an inline object schema");
        };
        assert!(object.properties.is_empty() && object.required.is_empty());
        assert!(matches!(
            object.additional_properties.as_deref(),
            Some(utoipa::openapi::schema::AdditionalProperties::FreeForm(
                false
            ))
        ));
        assert_eq!(NullSettings::name(), "NullSettings");
    }
}
