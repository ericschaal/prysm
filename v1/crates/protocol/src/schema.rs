//! OpenAPI roots for the wire values, and the hook hosts use to type the
//! adapter sections of [`Settings`](prysm_ports::Settings).
//!
//! `prysm_ports` stays free of OpenAPI, so its serialized types are described
//! here by schema-only mirrors. Tests keep each mirror's serde shape identical
//! to the ports type it describes.
//!
//! # Adapter schemas
//!
//! An adapter that wants a typed settings panel derives (or implements)
//! [`utoipa::ToSchema`] for its factory's `Settings` type, usually behind its
//! own `schema` feature. The schema describes the adapter's fields only; the
//! `kind` tag is added here from the factory's `KIND`, so the two cannot drift.
//! The host then lists the factories it registers:
//!
//! ```ignore
//! let document = Contract::new("Prysm server")
//!     .source::<DeviceSource>()
//!     .output::<Wled>()
//!     .document()?;
//! ```
//!
//! Without registrations `CaptureSettings` and `OutputSettings` are open
//! objects with a string `kind`.
use crate::{
    Failure, FailureKind, Phase, PreviewColor, PreviewColors, PreviewEvent, PreviewFrame,
    PreviewViewport, Session, SettingsReplaced, SettingsRequest, State, TargetRequest,
};
use prysm_ports::{OutputFactory, SourceFactory};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;
pub use utoipa::openapi::OpenApi;
use utoipa::openapi::schema::{
    AdditionalProperties, AllOfBuilder, ObjectBuilder, OneOfBuilder, Schema, Type,
};
use utoipa::openapi::{Ref, RefOr};
use utoipa::{OpenApi as _, PartialSchema, ToSchema};

// Mirrors `prysm_ports::Settings`.
/// Everything a user can change at runtime; adapter sections are tagged by `kind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[schema(as = Settings)]
#[serde(deny_unknown_fields)]
pub struct SettingsSchema {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    capture: Option<CaptureSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    output: Option<OutputSettings>,
    processing: ProcessingSchema,
}

// Mirrors `prysm_ports::Processing`.
/// Processing controls in plain units.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[schema(as = Processing)]
#[serde(deny_unknown_fields)]
pub struct ProcessingSchema {
    #[schema(minimum = 0, maximum = 100)]
    brightness_percent: u8,
    #[schema(minimum = 0, maximum = 100)]
    shadow_lift_percent: u8,
    /// Seconds; 0 disables smoothing.
    #[schema(minimum = 0)]
    smoothing_seconds: f32,
    #[schema(minimum = 1)]
    sample_density: u16,
    #[schema(minimum = 1, maximum = 100)]
    spatial_reach_percent: u8,
    remove_black_bars: bool,
    /// Saved color corrections; ids are unique.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(max_items = 32)]
    profiles: Vec<ColorProfileSchema>,
    /// The `id` of the applied profile; absent applies no correction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(
        nullable = false,
        min_length = 1,
        max_length = 64,
        pattern = "^[A-Za-z0-9_-]+$"
    )]
    active_profile: Option<String>,
}

// Mirrors `prysm_ports::ColorProfile`.
/// A named color correction, such as one per console.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[schema(as = ColorProfile)]
#[serde(deny_unknown_fields)]
pub struct ColorProfileSchema {
    /// Stable reference for `active_profile`; a UUID fits.
    #[schema(min_length = 1, max_length = 64, pattern = "^[A-Za-z0-9_-]+$")]
    id: String,
    /// Without surrounding whitespace or control characters.
    #[schema(min_length = 1, max_length = 64)]
    name: String,
    correction: CorrectionControlsSchema,
}

// Mirrors `prysm_ports::CorrectionControls`.
/// Correction controls, each -100 to 100; all zero is no correction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[schema(as = CorrectionControls)]
#[serde(deny_unknown_fields)]
pub struct CorrectionControlsSchema {
    /// Positive warms, negative cools.
    #[schema(minimum = -100, maximum = 100)]
    warmth: i16,
    /// Positive adds magenta, negative adds green.
    #[schema(minimum = -100, maximum = 100)]
    tint: i16,
    /// Positive lightens midtones, negative darkens them.
    #[schema(minimum = -100, maximum = 100)]
    midtones: i16,
    /// Positive saturates, negative desaturates.
    #[schema(minimum = -100, maximum = 100)]
    intensity: i16,
    hue: HueControlsSchema,
}

// Mirrors `prysm_ports::HueControls`.
/// Hue shift per wheel anchor, each -100 to 100.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[schema(as = HueControls)]
#[serde(deny_unknown_fields)]
pub struct HueControlsSchema {
    #[schema(minimum = -100, maximum = 100)]
    red: i16,
    #[schema(minimum = -100, maximum = 100)]
    yellow: i16,
    #[schema(minimum = -100, maximum = 100)]
    green: i16,
    #[schema(minimum = -100, maximum = 100)]
    cyan: i16,
    #[schema(minimum = -100, maximum = 100)]
    blue: i16,
    #[schema(minimum = -100, maximum = 100)]
    magenta: i16,
}

// Mirrors `prysm_ports::LightingTarget`.
/// What the user wants the LEDs to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = LightingTarget)]
#[serde(rename_all = "snake_case")]
pub enum LightingTargetSchema {
    Off,
    Processed,
    DeviceNative,
}

/// A capture section, `{ "kind": "...", ...fields }`. Replaced by the
/// registered adapters' tagged schemas in [`Contract::document`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureSettings(AdapterSection);

/// An output section; see [`CaptureSettings`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputSettings(AdapterSection);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct AdapterSection {
    kind: String,
    #[serde(flatten)]
    fields: Map<String, Value>,
}

fn open_section() -> RefOr<Schema> {
    ObjectBuilder::new()
        .property("kind", ObjectBuilder::new().schema_type(Type::String))
        .required("kind")
        .additional_properties(Some(AdditionalProperties::FreeForm(true)))
        .into()
}

impl PartialSchema for CaptureSettings {
    fn schema() -> RefOr<Schema> {
        open_section()
    }
}
impl ToSchema for CaptureSettings {}

impl PartialSchema for OutputSettings {
    fn schema() -> RefOr<Schema> {
        open_section()
    }
}
impl ToSchema for OutputSettings {}

#[derive(utoipa::OpenApi)]
#[openapi(components(schemas(
    State,
    Session,
    Phase,
    Failure,
    FailureKind,
    TargetRequest,
    SettingsRequest,
    SettingsReplaced,
    SettingsSchema,
    ProcessingSchema,
    ColorProfileSchema,
    CorrectionControlsSchema,
    HueControlsSchema,
    LightingTargetSchema,
    CaptureSettings,
    OutputSettings,
    PreviewEvent,
    PreviewFrame,
    PreviewViewport,
    PreviewColors,
    PreviewColor,
)))]
struct Roots;

type Schemas = Vec<(String, RefOr<Schema>)>;

struct Variant {
    kind: &'static str,
    name: String,
    schema: RefOr<Schema>,
    dependencies: Schemas,
}

impl Variant {
    fn of<T: ToSchema>(kind: &'static str) -> Self {
        let mut dependencies = Vec::new();
        T::schemas(&mut dependencies);
        Self {
            kind,
            name: T::name().into_owned(),
            schema: T::schema(),
            dependencies,
        }
    }

    /// The adapter's schema with `kind` added: as a property when it is a
    /// plain object, otherwise as an `allOf` with a tag object.
    fn tagged(self) -> RefOr<Schema> {
        let tag = ObjectBuilder::new()
            .schema_type(Type::String)
            .enum_values(Some([self.kind]));
        match self.schema {
            RefOr::T(Schema::Object(mut object)) => {
                object.properties.insert("kind".into(), tag.into());
                object.required.insert(0, "kind".into());
                RefOr::T(Schema::Object(object))
            }
            other => AllOfBuilder::new()
                .item(ObjectBuilder::new().property("kind", tag).required("kind"))
                .item(other)
                .into(),
        }
    }
}

/// One host's OpenAPI document: the wire roots plus its adapters' settings.
pub struct Contract {
    title: String,
    sources: Vec<Variant>,
    outputs: Vec<Variant>,
}

impl Contract {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            sources: Vec::new(),
            outputs: Vec::new(),
        }
    }

    /// Type `settings.capture` with this source's settings, tagged `F::KIND`.
    #[must_use]
    pub fn source<F: SourceFactory>(mut self) -> Self
    where
        F::Settings: ToSchema,
    {
        self.sources.push(Variant::of::<F::Settings>(F::KIND));
        self
    }

    /// Type `settings.output` with this output's settings, tagged `F::KIND`.
    #[must_use]
    pub fn output<F: OutputFactory>(mut self) -> Self
    where
        F::Settings: ToSchema,
    {
        self.outputs.push(Variant::of::<F::Settings>(F::KIND));
        self
    }

    /// # Errors
    /// Two schemas with the same name but different content.
    pub fn document(self) -> Result<OpenApi, SchemaConflict> {
        let mut document = Roots::openapi();
        document.info.title = self.title;
        document.info.version = env!("CARGO_PKG_VERSION").into();
        let schemas = &mut document
            .components
            .get_or_insert_with(Default::default)
            .schemas;
        for (section, variants) in [
            (CaptureSettings::name(), self.sources),
            (OutputSettings::name(), self.outputs),
        ] {
            if variants.is_empty() {
                continue;
            }
            let mut one_of = OneOfBuilder::new();
            for mut variant in variants {
                one_of = one_of.item(Ref::from_schema_name(variant.name.clone()));
                for (name, schema) in std::mem::take(&mut variant.dependencies) {
                    insert(schemas, name, schema)?;
                }
                insert(schemas, variant.name.clone(), variant.tagged())?;
            }
            schemas.insert(section.into_owned(), one_of.into());
        }
        Ok(document)
    }
}

fn insert(
    schemas: &mut std::collections::BTreeMap<String, RefOr<Schema>>,
    name: String,
    schema: RefOr<Schema>,
) -> Result<(), SchemaConflict> {
    match schemas.get(&name) {
        Some(existing) if *existing != schema => Err(SchemaConflict(name)),
        _ => {
            schemas.insert(name, schema);
            Ok(())
        }
    }
}

/// Two different schemas were registered under one name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaConflict(pub String);

impl fmt::Display for SchemaConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "conflicting schemas named `{}`", self.0)
    }
}

impl std::error::Error for SchemaConflict {}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_ports::{
        AdapterSettings, ColorProfile, CorrectionControls, FrameSource, HueControls,
        LightingTarget, Processing, Result, Settings, ValidationError,
    };
    use serde_json::json;

    /// Each mirror reads what the ports type writes, and vice versa.
    fn same_shape<P, M>(value: &P)
    where
        P: Serialize + serde::de::DeserializeOwned + PartialEq + fmt::Debug,
        M: Serialize + serde::de::DeserializeOwned,
    {
        let json = serde_json::to_value(value).unwrap();
        let mirror: M = serde_json::from_value(json.clone()).unwrap();
        let back = serde_json::to_value(&mirror).unwrap();
        assert_eq!(back, json);
        assert_eq!(&serde_json::from_value::<P>(back).unwrap(), value);
    }

    #[test]
    fn mirrors_match_ports_serialization() {
        same_shape::<Settings, SettingsSchema>(&Settings::default());
        same_shape::<Settings, SettingsSchema>(&Settings {
            capture: Some(AdapterSettings::new(
                "device",
                &json!({ "path": "/dev/video0" }),
            )),
            output: Some(AdapterSettings::new(
                "wled",
                &json!({ "address": "10.0.0.5" }),
            )),
            processing: Processing::default(),
        });
        same_shape::<Settings, SettingsSchema>(&Settings {
            processing: Processing {
                profiles: vec![
                    ColorProfile {
                        id: "ps5".into(),
                        name: "PS5".into(),
                        correction: CorrectionControls {
                            warmth: 12,
                            tint: -4,
                            midtones: 3,
                            intensity: 20,
                            hue: HueControls::from_array([1, -2, 3, -4, 5, -6]),
                        },
                    },
                    ColorProfile {
                        id: "0b6f2c1e-1d2a-4c3b-9f4e-5a6b7c8d9e0f".into(),
                        name: "TV apps".into(),
                        correction: CorrectionControls::default(),
                    },
                ],
                active_profile: Some("ps5".into()),
                ..Processing::default()
            },
            ..Settings::default()
        });
        for target in [
            LightingTarget::Off,
            LightingTarget::Processed,
            LightingTarget::DeviceNative,
        ] {
            same_shape::<LightingTarget, LightingTargetSchema>(&target);
        }
    }

    #[derive(Serialize, Deserialize, ToSchema)]
    struct DemoSettings {
        path: String,
    }

    struct Demo;
    impl SourceFactory for Demo {
        const KIND: &'static str = "demo";
        type Settings = DemoSettings;
        fn validate(&self, _: &DemoSettings) -> Result<(), ValidationError> {
            Ok(())
        }
        fn open(&self, _: &DemoSettings) -> Result<Box<dyn FrameSource>> {
            Err(prysm_ports::Error::other("demo"))
        }
    }

    fn schema_json(document: &OpenApi, name: &str) -> Value {
        serde_json::to_value(&document.components.as_ref().unwrap().schemas[name]).unwrap()
    }

    #[test]
    fn registered_sources_become_kind_tagged_variants() {
        let document = Contract::new("test").source::<Demo>().document().unwrap();
        assert_eq!(
            schema_json(&document, "CaptureSettings"),
            json!({ "oneOf": [{ "$ref": "#/components/schemas/DemoSettings" }] })
        );
        let demo = schema_json(&document, "DemoSettings");
        assert_eq!(demo["properties"]["kind"]["enum"], json!(["demo"]));
        assert_eq!(demo["required"], json!(["kind", "path"]));
        // Unregistered sections stay open.
        assert_eq!(
            schema_json(&document, "OutputSettings")["required"],
            json!(["kind"])
        );
    }

    #[test]
    fn conflicting_names_are_rejected() {
        let result = Contract::new("test")
            .source::<Demo>()
            .source::<Other>()
            .document();
        assert_eq!(result.err(), Some(SchemaConflict("DemoSettings".into())));
    }

    struct Other;
    impl SourceFactory for Other {
        const KIND: &'static str = "other";
        type Settings = DemoSettings;
        fn validate(&self, _: &DemoSettings) -> Result<(), ValidationError> {
            Ok(())
        }
        fn open(&self, _: &DemoSettings) -> Result<Box<dyn FrameSource>> {
            Err(prysm_ports::Error::other("other"))
        }
    }
}
