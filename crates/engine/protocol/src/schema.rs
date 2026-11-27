//! OpenAPI roots for the wire values, and the hook hosts use to type the
//! adapter sections of [`Settings`](prysm_ports::Settings).
//!
//! Ports provides schemas for its wire types under the optional `schema` feature.
//! Serialization mirrors below describe the remaining protocol-specific shapes.
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
    ColorsEvent, Failure, PreviewEvent, SettingsReplaced, SettingsRequest, State, TargetRequest,
};
use prysm_ports::{
    AdapterSettings, MAX_CALIBRATION_ID_LEN, MAX_CALIBRATION_NAME_CHARS, MAX_CALIBRATIONS,
    OutputFactory, OutputIssue, SourceFactory,
};
use serde::{Deserialize, Serialize};
pub use utoipa::openapi::OpenApi;
use utoipa::openapi::schema::{
    AdditionalProperties, AllOfBuilder, ObjectBuilder, OneOfBuilder, Schema, Type,
};
use utoipa::openapi::{Ref, RefOr};
use utoipa::{OpenApi as _, PartialSchema, ToSchema};

/// The HTTP API's own version (`/api/v1`), in the documents' `info.version`;
/// not the product version, which only `VERSION` carries.
pub const API_VERSION: &str = "1";

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

// `schema(...)` takes literals only: the calibration, detail, smoothing, ramp and floor limits below are spelled out.
const _: () = assert!(
    MAX_CALIBRATIONS == 32 && MAX_CALIBRATION_ID_LEN == 64 && MAX_CALIBRATION_NAME_CHARS == 64
);
const _: () = assert!(prysm_ports::Processing::MAX_SAMPLE_DENSITY == 512);
const _: () = assert!(prysm_ports::StyleValues::MAX_SMOOTHING_SECONDS == 2.0);
const _: () = assert!(prysm_ports::StyleValues::MAX_BRIGHTNESS_RAMP_SECONDS == 2.0);
const _: () = assert!(prysm_ports::StyleValues::MAX_DARK_FLOOR_PERCENT == 20);

// Mirrors `prysm_ports::Processing`.
/// Processing controls in plain units.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[schema(as = Processing)]
#[serde(deny_unknown_fields)]
pub struct ProcessingSchema {
    #[schema(format = "uint8", minimum = 0, maximum = 100)]
    brightness_percent: u8,
    style: ChosenStyleSchema,
    /// The user's own values, applied while the style is `custom` and kept
    /// while another is chosen; absent until first tuned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    custom_style: Option<StyleValuesSchema>,
    /// Samples per 1000 px of edge, measured on the picture scaled down to
    /// 640x360: the same value gives the same samples at any capture size.
    /// At most one sample per sampling-grid cell along an edge (128 along the
    /// longer side): density 200 reaches it for 16:9 pictures of 640x360 or
    /// more, a higher one for squarer or smaller pictures.
    #[schema(format = "uint16", minimum = 1, maximum = 512)]
    sample_density: u16,
    remove_black_bars: bool,
    /// Saved color corrections; ids are unique.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(max_items = 32)]
    calibrations: Vec<CalibrationSchema>,
    /// The `id` of the applied calibration; absent applies no correction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(
        nullable = false,
        min_length = 1,
        max_length = 64,
        pattern = "^[A-Za-z0-9_-]+$"
    )]
    active_calibration: Option<String>,
}

// Mirrors `prysm_ports::style::as_object`.
/// The chosen style, as `Processing.style`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = ChosenStyle)]
#[serde(deny_unknown_fields)]
pub struct ChosenStyleSchema {
    kind: StyleSchema,
}

// Mirrors `prysm_ports::StyleValues`.
/// The picture-taste knobs a style sets, in plain units.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ToSchema)]
#[schema(as = StyleValues)]
#[serde(deny_unknown_fields)]
pub struct StyleValuesSchema {
    /// Seconds to complete most of a change; 0 disables smoothing.
    #[schema(minimum = 0, maximum = 2)]
    smoothing_seconds: f32,
    /// How much of a scene cut the lights take at once; 0 eases a cut in
    /// like any other change.
    #[schema(format = "uint8", minimum = 0, maximum = 100)]
    cut_response_percent: u8,
    /// Seconds, the least time from dark to full brightness; 0 sets no limit.
    #[schema(minimum = 0, maximum = 2)]
    brightness_ramp_seconds: f32,
    /// Edge depth: how far into the picture each light looks, in percent of
    /// the picture's height.
    #[schema(format = "uint8", minimum = 1, maximum = 100)]
    spatial_reach_percent: u8,
    /// How much each light keeps the colour next to it, taking less from the
    /// rest of the picture; 0 is the plain average.
    #[schema(format = "uint8", minimum = 0, maximum = 100)]
    color_focus_percent: u8,
    /// Scales each light's chroma by 1 + p/100; 0 leaves colours as they are.
    #[schema(format = "uint8", minimum = 0, maximum = 100)]
    saturation_boost_percent: u8,
    /// The least luminance the lights keep, as a neutral white glow, in
    /// percent of full light; 0 lets them go fully dark.
    #[schema(format = "uint8", minimum = 0, maximum = 20)]
    dark_floor_percent: u8,
}

// Mirrors `prysm_ports::Calibration`.
/// A named color correction, such as one per console.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[schema(as = Calibration)]
#[serde(deny_unknown_fields)]
pub struct CalibrationSchema {
    /// Stable reference for `active_calibration`; a UUID fits.
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
    #[schema(format = "int16", minimum = -100, maximum = 100)]
    warmth: i16,
    /// Positive adds magenta, negative adds green.
    #[schema(format = "int16", minimum = -100, maximum = 100)]
    tint: i16,
    /// Positive lightens midtones, negative darkens them.
    #[schema(format = "int16", minimum = -100, maximum = 100)]
    midtones: i16,
    red: PrimaryControlsSchema,
    green: PrimaryControlsSchema,
    blue: PrimaryControlsSchema,
}

// Mirrors `prysm_ports::PrimaryControls`.
/// One picture primary's controls, each -100 to 100; white never moves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[schema(as = PrimaryControls)]
#[serde(deny_unknown_fields)]
pub struct PrimaryControlsSchema {
    /// Positive turns toward the next primary in red, green, blue order,
    /// negative toward the previous one.
    #[schema(format = "int16", minimum = -100, maximum = 100)]
    hue: i16,
    /// Negative makes the colour softer, positive stronger.
    #[schema(format = "int16", minimum = -100, maximum = 100)]
    strength: i16,
}

// Mirrors `prysm_ports::CalibrationPattern`.
/// A calibration pattern shown on the TV: the body of
/// `PUT /api/v1/calibration` and `State.calibration`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = CalibrationPattern)]
#[serde(deny_unknown_fields)]
pub struct CalibrationPatternSchema {
    pattern: PatternSchema,
    /// Apply no calibration while the pattern shows, to compare.
    uncorrected: bool,
}

// Mirrors `prysm_ports::Input`.
/// Whether frames are arriving, and how fast.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ToSchema)]
#[schema(as = Input)]
pub struct InputSchema {
    /// Frames per second over the last second, to one decimal.
    #[schema(minimum = 0, example = 29.9)]
    fps: f32,
    /// Whether frames arrived in the last second, and if not, why.
    picture: PictureSchema,
    /// Frames per second whose colours were handed to the output over the
    /// last second, to one decimal. An output may drop a frame it cannot
    /// send now; the device issue says when the lights do not answer.
    #[schema(minimum = 0, example = 29.9)]
    #[serde(default)]
    sent_fps: f32,
    /// The mean time per frame along the path, in two parts, over the last
    /// second. Absent when no frame reached the output.
    #[schema(nullable = false)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    latency: Option<LatencySchema>,
    /// Which step keeps Prysm from keeping up with its source, if one does.
    /// Absent while it keeps up.
    #[schema(nullable = false)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    behind: Option<BehindSchema>,
}

// Mirrors `prysm_ports::Behind` (`none` is not sent: the field is absent).
/// What makes Prysm fall behind its source: for several seconds the frame
/// loop had no slack, and this step took the larger share. A source that
/// just sends fewer frames is never this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = Behind)]
#[serde(rename_all = "snake_case")]
pub enum BehindSchema {
    /// Turning frames into colours takes most of the time.
    Processing,
    /// Writing the colours to the lights takes most of the time.
    Output,
}

// Mirrors `prysm_ports::DeviceReport`.
/// What the lights say about themselves, from the output's own checks. Each
/// part is absent while the output cannot tell, and all are while no session
/// holds the output (no session, no frame yet, or released after 3 s without
/// video; see `session.state` and `input.picture`), but `issue`
/// `unreachable` while the lights are waited for: lighting's open, or the
/// request for `off` or their own effects, that they did not answer. Prysm
/// reports them and changes none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = DeviceReport)]
pub struct DeviceReportSchema {
    /// Why the lights are not showing Prysm's colours, or that they do not
    /// answer; absent while all is well.
    #[schema(nullable = false)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    issue: Option<OutputIssue>,
    /// The lights' own brightness while it scales Prysm's colours (WLED: its
    /// master brightness and brightness factor): the share of Prysm's
    /// brightness they show. Absent while it does not (WLED: "Force max
    /// brightness" on).
    #[schema(nullable = false, minimum = 0, maximum = 100, example = 40)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    brightness_percent: Option<u8>,
    /// How many LEDs the lights say they have (WLED: its LED count). Prysm
    /// sends the configured `settings.output.leds` either way: a different
    /// count leaves LEDs dark or colours unshown.
    #[schema(nullable = false, maximum = 4_294_967_295_u64, example = 120)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    led_count: Option<u32>,
}

pub use prysm_ports::{
    Candidate as CandidateSchema, Latency as LatencySchema, LightingTarget as LightingTargetSchema,
    Mode as ModeSchema, ModesSchema, Pattern as PatternSchema, Picture as PictureSchema,
    SpanSchema, Style as StyleSchema,
};

/// A capture section, `{ "kind": "...", ...fields }`. Replaced by the
/// registered adapters' tagged schemas in [`Contract::document`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureSettings(AdapterSettings);

/// An output section; see [`CaptureSettings`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputSettings(AdapterSettings);

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
    TargetRequest,
    SettingsRequest,
    SettingsReplaced,
    Failure,
    PreviewEvent,
    ColorsEvent,
    CandidateSchema,
    ModesSchema,
)))]
struct Roots;

struct Variant {
    reference: Ref,
    schemas: Vec<(String, RefOr<Schema>)>,
}

impl Variant {
    fn of<T: ToSchema>(kind: &'static str) -> Self {
        let mut schemas = Vec::new();
        T::schemas(&mut schemas);
        let name = T::name().into_owned();
        let reference = Ref::from_schema_name(&name);
        let tag = ObjectBuilder::new()
            .schema_type(Type::String)
            .enum_values(Some([kind]));
        let tagged = match T::schema() {
            RefOr::T(Schema::Object(mut object)) => {
                object.properties.insert("kind".into(), tag.into());
                object.required.insert(0, "kind".into());
                RefOr::T(Schema::Object(object))
            }
            other => AllOfBuilder::new()
                .item(ObjectBuilder::new().property("kind", tag).required("kind"))
                .item(other)
                .into(),
        };
        schemas.push((name, tagged));
        Self { reference, schemas }
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
        document.info.version = API_VERSION.into();
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
            for variant in variants {
                one_of = one_of.item(variant.reference);
                for (name, schema) in variant.schemas {
                    insert(schemas, name, schema)?;
                }
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
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("conflicting schemas named `{0}`")]
pub struct SchemaConflict(pub String);

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_ports::{
        AdapterSettings, Behind, Calibration, CalibrationPattern, CorrectionControls,
        DeviceBrightness, DeviceReport, FrameSource, Input, Latency, Mode, Modes, OutputIssue,
        Pattern, Picture, PrimaryControls, Processing, Result, Settings, Span, Style, StyleValues,
        ValidationError,
    };
    use prysm_testkit::assert_mirror_round_trips;
    use serde_json::{Value, json};

    #[test]
    fn mirrors_match_ports_serialization() {
        assert_mirror_round_trips::<Settings, SettingsSchema>(&Settings::default());
        assert_mirror_round_trips::<Settings, SettingsSchema>(&Settings {
            capture: Some(AdapterSettings::new(
                "device",
                &json!({ "device": "/dev/video0" }),
            )),
            output: Some(AdapterSettings::new(
                "wled",
                &json!({ "address": "10.0.0.5" }),
            )),
            processing: Processing::default(),
        });
        assert_mirror_round_trips::<Settings, SettingsSchema>(&Settings {
            processing: Processing {
                calibrations: vec![
                    Calibration {
                        id: "ps5".into(),
                        name: "PS5".into(),
                        correction: CorrectionControls {
                            warmth: 12,
                            tint: -4,
                            midtones: 3,
                            red: PrimaryControls {
                                hue: 1,
                                strength: -2,
                            },
                            green: PrimaryControls {
                                hue: 3,
                                strength: -4,
                            },
                            blue: PrimaryControls {
                                hue: 5,
                                strength: -6,
                            },
                        },
                    },
                    Calibration {
                        id: "0b6f2c1e-1d2a-4c3b-9f4e-5a6b7c8d9e0f".into(),
                        name: "TV apps".into(),
                        correction: CorrectionControls::default(),
                    },
                ],
                active_calibration: Some("ps5".into()),
                ..Processing::default()
            },
            ..Settings::default()
        });
        assert_mirror_round_trips::<Settings, SettingsSchema>(&Settings {
            processing: Processing {
                style: Style::Custom,
                custom_style: Some(StyleValues {
                    cut_response_percent: 70,
                    brightness_ramp_seconds: 0.5,
                    color_focus_percent: 60,
                    saturation_boost_percent: 40,
                    dark_floor_percent: 10,
                    ..StyleValues::DEFAULT
                }),
                ..Processing::default()
            },
            ..Settings::default()
        });
        for style in Style::BUILT_IN {
            assert_mirror_round_trips::<StyleValues, StyleValuesSchema>(&style.built_in().unwrap());
        }
        assert_mirror_round_trips::<Input, InputSchema>(&Input {
            fps: 29.5,
            picture: Picture::Arriving,
            sent_fps: 29.5,
            latency: Some(Latency {
                capture_ms: Some(88.0),
                output_ms: 2.4,
            }),
            behind: Behind::Output,
        });
        assert_mirror_round_trips::<Input, InputSchema>(&Input {
            behind: Behind::Processing,
            ..Input::default()
        });
        for picture in [Picture::Arriving, Picture::Quiet, Picture::Refused] {
            assert_mirror_round_trips::<Input, InputSchema>(&Input {
                picture,
                ..Input::default()
            });
        }
        assert_mirror_round_trips::<Input, InputSchema>(&Input {
            latency: Some(Latency {
                capture_ms: None,
                output_ms: 4.0,
            }),
            ..Input::default()
        });
        assert_mirror_round_trips::<Input, InputSchema>(&Input::default());
        assert_mirror_round_trips::<DeviceReport, DeviceReportSchema>(&DeviceReport::default());
        assert_mirror_round_trips::<DeviceReport, DeviceReportSchema>(&DeviceReport {
            issue: Some(OutputIssue::OtherSender),
            brightness: DeviceBrightness::from_percent(40),
            led_count: Some(120),
        });
        assert_mirror_round_trips::<Modes, ModesSchema>(&Modes::Unknown);
        assert_mirror_round_trips::<Modes, ModesSchema>(&Modes::Known(Vec::new()));
        assert_mirror_round_trips::<Modes, ModesSchema>(&Modes::Known(vec![
            Mode::new(
                Span::new(2, 1920, 2).unwrap(),
                Span::new(2, 1080, 2).unwrap(),
                Span::new(1, 60, 1).unwrap(),
            ),
            Mode::new(Span::exactly(640), Span::exactly(360), Span::exactly(30)),
        ]));
        for (index, pattern) in Pattern::ALL.into_iter().enumerate() {
            assert_mirror_round_trips::<CalibrationPattern, CalibrationPatternSchema>(
                &CalibrationPattern {
                    pattern,
                    uncorrected: index % 2 == 0,
                },
            );
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
        assert_eq!(
            schema_json(&document, "OutputSettings")["required"],
            json!(["kind"])
        );
    }

    #[test]
    fn a_composite_adapter_keeps_its_schema_and_dependencies() {
        #[derive(ToSchema)]
        #[allow(dead_code)]
        enum Feed {
            File(DemoSettings),
            Off,
        }

        let document = Contract {
            sources: vec![Variant::of::<Feed>("feed")],
            ..Contract::new("test")
        }
        .document()
        .unwrap();
        let original = serde_json::to_value(Feed::schema()).unwrap();
        assert!(original.get("oneOf").is_some());
        assert_eq!(
            schema_json(&document, "Feed")["allOf"],
            json!([
                {"type": "object", "required": ["kind"], "properties": {"kind": {"type": "string", "enum": ["feed"]}}},
                original,
            ])
        );
        assert_eq!(
            schema_json(&document, "DemoSettings")["required"],
            json!(["path"])
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
