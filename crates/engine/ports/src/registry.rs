use crate::{
    AdapterSettings, Error, FrameSource, Hangup, Modes, OpenIntent, OutputObservationState,
    OutputOpen, Power, Readiness, Result, Section, Settings, ValidationError,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

/// The longest a [`SourceFactory::open`] may take, releasing what a failed
/// open held included. A command that (re)starts lighting waits for a
/// session's stop and one source open, so this and the stop deadline bound
/// every command (`prysm_runtime::COMMAND_LIMIT`). `device` waits in its
/// open (for the device to be ready, then to release a failed one) and
/// asserts that those bounds fit. `webos`, `file` and `remote` open without
/// waiting for hardware or a peer: they load libraries, start a thread or
/// spawn FFmpeg and return, and wait for frames only once pulled, so they
/// need no assertion.
pub const SOURCE_OPEN_LIMIT: Duration = Duration::from_secs(13);

/// Opens a [`FrameSource`] from typed settings. Opening may block.
pub trait SourceFactory: Send + Sync + 'static {
    /// Tag in the settings file, such as `"device"` or `"webos"`.
    const KIND: &'static str;
    type Settings: DeserializeOwned + Serialize + Send + Sync;
    /// # Errors
    /// Rejects settings that can never open, without touching hardware. The
    /// field is relative to the section, such as `fps`: the registry names
    /// the section.
    fn validate(&self, settings: &Self::Settings) -> Result<(), ValidationError>;
    /// # Errors
    /// Device or I/O failure, or a size or rate [`Self::modes`] says the
    /// hardware does not deliver.
    fn open(&self, settings: &Self::Settings) -> Result<Box<dyn FrameSource>>;
    /// Open for an owned attempt, without opening after a pre-existing cut.
    fn open_for_sync(
        &self,
        settings: &Self::Settings,
        stop: &Hangup,
    ) -> Result<Box<dyn FrameSource>> {
        if stop.is_cut() {
            return Err(Error::adapter("capture open cancelled", crate::Stopping));
        }
        self.open(settings)
    }
    /// Check current readiness without starting a session.
    fn check(&self, _settings: &Self::Settings, _stop: &Hangup) -> Readiness<Error> {
        Readiness::Ready
    }
    /// Classify an open or held-handle failure using adapter knowledge.
    fn failure(&self, error: Error) -> Readiness<Error> {
        Readiness::Blocked(error)
    }
    /// Wait for a relevant readiness change, or return None when cancelled.
    fn watch(&self, _settings: &Self::Settings, _stop: &Hangup) -> Option<Readiness<Error>> {
        None
    }
    /// What this kind finds to choose from, such as the video devices present.
    /// See [`Candidate`]; the default finds nothing.
    fn candidates(&self) -> Vec<Candidate> {
        Vec::new()
    }
    /// The sizes and rates this source delivers with `settings`, as
    /// [`Modes`] says: ranges where the hardware takes any value in them,
    /// concrete modes where it offers fixed ones. Only the fields that select hardware
    /// matter (a device path, an encoding), not the size and rate asked for,
    /// which need not be valid. May block briefly, like
    /// [`Self::candidates`]; the default cannot say. `open` refuses what
    /// this excludes, naming the field.
    fn modes(&self, settings: &Self::Settings) -> Modes {
        let _ = settings;
        Modes::Unknown
    }
}

/// Opens a [`LedOutput`] from typed settings. Opening may block.
pub trait OutputFactory: Send + Sync + 'static {
    const KIND: &'static str;
    type Settings: DeserializeOwned + Serialize + Send + Sync;
    /// # Errors
    /// Rejects settings that can never open, without touching hardware. The
    /// field is relative to the section, such as `address`: the registry
    /// names the section.
    fn validate(&self, settings: &Self::Settings) -> Result<(), ValidationError>;
    /// Start streaming with the permission carried by this request. Ends
    /// when `stop` is cut. An open failure records whether its request may
    /// have reached the device.
    fn open_for_sync(
        &self,
        settings: &Self::Settings,
        intent: OpenIntent,
        stop: &Hangup,
    ) -> Result<OutputOpen, OpenFailure>;
    /// Check current readiness and optional physical device facts.
    fn observe(
        &self,
        _settings: &Self::Settings,
        _stop: &Hangup,
    ) -> (Readiness<Error>, OutputObservationState) {
        (Readiness::Ready, OutputObservationState::Unsupported)
    }
    /// Classify an open, held-handle or power failure.
    fn failure(&self, error: Error) -> Readiness<Error> {
        Readiness::Blocked(error)
    }
    /// Wait for a relevant readiness change, or return None when cancelled.
    fn watch(
        &self,
        _settings: &Self::Settings,
        _stop: &Hangup,
    ) -> Option<(Readiness<Error>, OutputObservationState)> {
        None
    }
    /// Whether this output can delegate display to its own effects.
    fn supports_effects(&self) -> bool {
        false
    }
    /// Power request, independent of releasing a local stream.
    fn set_power(&self, settings: &Self::Settings, power: Power, stop: &Hangup) -> Result<()>;
    /// What this kind finds to choose from, such as devices on the network.
    /// See [`Candidate`]; the default finds nothing.
    fn candidates(&self) -> Vec<Candidate> {
        Vec::new()
    }
}

/// Why an output did not open, and what that leaves its device in.
#[derive(Debug, Clone)]
pub enum OpenFailure {
    /// Nothing reached the device (invalid settings, a name that does not
    /// resolve, a local socket, a refused connection): it is as it was.
    Untouched(Error),
    /// The device may have taken a request: what it shows is unknown.
    Touched(Error),
}

impl OpenFailure {
    #[must_use]
    pub fn into_error(self) -> Error {
        match self {
            Self::Untouched(error) | Self::Touched(error) => error,
        }
    }

    /// The same failure with its error changed by `change`.
    #[must_use]
    pub fn map(self, change: impl FnOnce(Error) -> Error) -> Self {
        match self {
            Self::Untouched(error) => Self::Untouched(change(error)),
            Self::Touched(error) => Self::Touched(change(error)),
        }
    }
}

/// Something an adapter found that a person can pick instead of typing, one
/// item of `GET /api/v1/candidates/{section}/{kind}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Candidate {
    /// What a person recognises, such as the device's name.
    pub label: String,
    /// Where it is or what it has, such as `192.168.1.40`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schema(nullable = false))]
    pub detail: Option<String>,
    /// How many LEDs a found output device says it has, when it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "schema",
        schema(nullable = false, maximum = 4_294_967_295_u64, example = 120)
    )]
    pub led_count: Option<u32>,
    /// The settings fields picking it fills, such as `{ "device": "/dev/video0" }`: part of the kind's settings, merged over what the person has.
    #[cfg_attr(feature = "schema", schema(additional_properties))]
    pub fields: Map<String, Value>,
}

impl Candidate {
    /// # Panics
    /// If `fields` does not serialize to a JSON object, which is a bug in the adapter.
    pub fn new(label: impl Into<String>, fields: &impl Serialize) -> Self {
        let Ok(Value::Object(fields)) = serde_json::to_value(fields) else {
            panic!("candidate fields must serialize to a JSON object");
        };
        Self {
            label: label.into(),
            detail: None,
            led_count: None,
            fields,
        }
    }

    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    #[must_use]
    pub fn led_count(mut self, count: u32) -> Self {
        self.led_count = Some(count);
        self
    }
}

/// The adapters a host offers, keyed by kind. Erases factory types so the
/// engine and the settings file need no generics.
#[derive(Clone, Default)]
pub struct Registry {
    sources: BTreeMap<&'static str, Arc<dyn ErasedSource>>,
    outputs: BTreeMap<&'static str, Arc<dyn ErasedOutput>>,
}

impl Registry {
    /// # Panics
    /// If a source of the same kind is already registered.
    pub fn add_source<F: SourceFactory>(&mut self, factory: F) {
        let previous = self.sources.insert(F::KIND, Arc::new(factory));
        assert!(previous.is_none(), "duplicate source kind `{}`", F::KIND);
    }

    /// # Panics
    /// If an output of the same kind is already registered.
    pub fn add_output<F: OutputFactory>(&mut self, factory: F) {
        let previous = self.outputs.insert(F::KIND, Arc::new(factory));
        assert!(previous.is_none(), "duplicate output kind `{}`", F::KIND);
    }

    /// Validate processing (including calibrations and the active calibration)
    /// and every configured adapter against its factory.
    ///
    /// # Errors
    /// [`Error::Invalid`] or [`Error::Decode`].
    pub fn validate(&self, settings: &Settings) -> Result<()> {
        settings.processing.validate()?;
        if let Some(capture) = &settings.capture {
            self.source(capture)?;
        }
        if let Some(output) = &settings.output {
            self.output(output)?;
        }
        Ok(())
    }

    /// Resolve capture settings into a port. All later calls use these typed settings.
    pub fn source(&self, settings: &AdapterSettings) -> Result<Arc<dyn ConfiguredSource>> {
        find(&self.sources, Section::Capture, &settings.kind)?
            .clone()
            .bind(settings)
            .map_err(|error| within(Section::Capture, error))
    }

    /// Resolve output settings without reaching the device.
    pub fn output(&self, settings: &AdapterSettings) -> Result<Arc<dyn ConfiguredOutput>> {
        find(&self.outputs, Section::Output, &settings.kind)?
            .clone()
            .bind(settings)
            .map_err(|error| within(Section::Output, error))
    }

    /// A registered capability, independent of whether saved settings are usable.
    pub fn supports_effects(&self, settings: &AdapterSettings) -> bool {
        self.outputs
            .get(settings.kind.as_str())
            .is_some_and(|factory| factory.supports_effects())
    }

    /// What the factory registered for `kind` in `section` finds to choose
    /// from. Blocking, for up to about two seconds; see [`Candidate`].
    ///
    /// # Errors
    /// [`Error::Invalid`] (field `<section>.kind`) for a kind not registered
    /// in `section`, or for the processing section, which has no kinds.
    /// Discovery is on demand. It finishes in about two seconds and retains no open devices, threads, or caches.
    pub fn candidates(&self, section: Section, kind: &str) -> Result<Vec<Candidate>> {
        Ok(match section {
            Section::Capture => find(&self.sources, section, kind)?.candidates(),
            Section::Output => find(&self.outputs, section, kind)?.candidates(),
            Section::Processing => {
                return Err(ValidationError::new("processing", "has no adapter kinds").into());
            }
        })
    }

    /// What the capture `settings` select delivers; see
    /// [`SourceFactory::modes`]. Blocking, briefly.
    ///
    /// # Errors
    /// Unknown kind (field `capture.kind`) or undecodable settings.
    pub fn capture_modes(&self, settings: &AdapterSettings) -> Result<Modes> {
        let section = Section::Capture;
        find(&self.sources, section, &settings.kind)?
            .modes(settings)
            .map_err(|error| within(section, error))
    }
}

/// A factory's invalid or undecodable field, relative to its section, as a
/// full path; an undecodable section as a whole is the section's name.
fn within(section: Section, error: Error) -> Error {
    match error {
        Error::Invalid(ValidationError { field, reason }) => Error::Invalid(ValidationError {
            field: format!("{section}.{field}"),
            reason,
        }),
        Error::Decode {
            kind,
            field,
            source,
        } => Error::Decode {
            kind,
            field: if field.is_empty() {
                section.to_string()
            } else {
                format!("{section}.{field}")
            },
            source,
        },
        other => other,
    }
}

fn find<'a, F: ?Sized>(
    map: &'a BTreeMap<&'static str, Arc<F>>,
    section: Section,
    kind: &str,
) -> Result<&'a Arc<F>> {
    map.get(kind).ok_or_else(|| {
        ValidationError::new(format!("{section}.kind"), format!("unknown kind `{kind}`")).into()
    })
}

/// Prerequisites and recovery for one immutable adapter configuration.
pub trait ConfiguredAdapter: Send + Sync {
    fn observe(&self, stop: &Hangup) -> (Readiness<Error>, OutputObservationState);
    fn watch(&self, stop: &Hangup) -> Option<(Readiness<Error>, OutputObservationState)>;
    fn failure(&self, error: Error) -> Readiness<Error>;
}

pub trait ConfiguredSource: ConfiguredAdapter {
    fn open(&self, stop: &Hangup) -> Result<Box<dyn FrameSource>>;
}

pub trait ConfiguredOutput: ConfiguredAdapter {
    fn open(&self, intent: OpenIntent, stop: &Hangup) -> Result<OutputOpen, OpenFailure>;
    fn set_power(&self, power: Power, stop: &Hangup) -> Result<()>;
}

trait ErasedSource: Send + Sync {
    fn bind(self: Arc<Self>, settings: &AdapterSettings) -> Result<Arc<dyn ConfiguredSource>>;
    fn candidates(&self) -> Vec<Candidate>;
    fn modes(&self, settings: &AdapterSettings) -> Result<Modes>;
}

trait ErasedOutput: Send + Sync {
    fn bind(self: Arc<Self>, settings: &AdapterSettings) -> Result<Arc<dyn ConfiguredOutput>>;
    fn candidates(&self) -> Vec<Candidate>;
    fn supports_effects(&self) -> bool;
}

/// The fields as the factory's type; a failure names the field relative to
/// the section (empty for the section as a whole), for [`within`] to complete.
fn decode<S: DeserializeOwned>(settings: &AdapterSettings) -> Result<S> {
    let fields = Value::Object(settings.fields.clone());
    serde_path_to_error::deserialize(fields).map_err(|error| {
        let path = error.path();
        Error::Decode {
            kind: settings.kind.clone(),
            field: if path.iter().next().is_none() {
                String::new()
            } else {
                path.to_string()
            },
            source: Arc::new(error.into_inner()),
        }
    })
}

struct Source<F: SourceFactory> {
    factory: Arc<F>,
    settings: F::Settings,
}

struct Output<F: OutputFactory> {
    factory: Arc<F>,
    settings: F::Settings,
}

impl<F: SourceFactory> ErasedSource for F {
    fn bind(self: Arc<Self>, raw: &AdapterSettings) -> Result<Arc<dyn ConfiguredSource>> {
        let settings = decode(raw)?;
        self.validate(&settings)?;
        Ok(Arc::new(Source {
            factory: self,
            settings,
        }))
    }
    fn candidates(&self) -> Vec<Candidate> {
        SourceFactory::candidates(self)
    }
    fn modes(&self, raw: &AdapterSettings) -> Result<Modes> {
        Ok(SourceFactory::modes(self, &decode(raw)?))
    }
}

impl<F: OutputFactory> ErasedOutput for F {
    fn bind(self: Arc<Self>, raw: &AdapterSettings) -> Result<Arc<dyn ConfiguredOutput>> {
        let settings = decode(raw)?;
        self.validate(&settings)?;
        Ok(Arc::new(Output {
            factory: self,
            settings,
        }))
    }
    fn candidates(&self) -> Vec<Candidate> {
        OutputFactory::candidates(self)
    }
    fn supports_effects(&self) -> bool {
        OutputFactory::supports_effects(self)
    }
}

impl<F: SourceFactory> ConfiguredAdapter for Source<F> {
    fn observe(&self, stop: &Hangup) -> (Readiness<Error>, OutputObservationState) {
        let ready = self
            .factory
            .check(&self.settings, stop)
            .map(|err| within(Section::Capture, err));
        (ready, OutputObservationState::Unsupported)
    }
    fn watch(&self, stop: &Hangup) -> Option<(Readiness<Error>, OutputObservationState)> {
        let ready = self
            .factory
            .watch(&self.settings, stop)?
            .map(|err| within(Section::Capture, err));
        Some((ready, OutputObservationState::Unsupported))
    }
    fn failure(&self, error: Error) -> Readiness<Error> {
        self.factory.failure(error)
    }
}

impl<F: SourceFactory> ConfiguredSource for Source<F> {
    fn open(&self, stop: &Hangup) -> Result<Box<dyn FrameSource>> {
        self.factory
            .open_for_sync(&self.settings, stop)
            .map_err(|err| within(Section::Capture, err))
    }
}

impl<F: OutputFactory> ConfiguredAdapter for Output<F> {
    fn observe(&self, stop: &Hangup) -> (Readiness<Error>, OutputObservationState) {
        let (ready, facts) = self.factory.observe(&self.settings, stop);
        (ready.map(|err| within(Section::Output, err)), facts)
    }
    fn watch(&self, stop: &Hangup) -> Option<(Readiness<Error>, OutputObservationState)> {
        let (ready, facts) = self.factory.watch(&self.settings, stop)?;
        Some((ready.map(|err| within(Section::Output, err)), facts))
    }
    fn failure(&self, error: Error) -> Readiness<Error> {
        self.factory.failure(error)
    }
}

impl<F: OutputFactory> ConfiguredOutput for Output<F> {
    fn open(&self, intent: OpenIntent, stop: &Hangup) -> Result<OutputOpen, OpenFailure> {
        self.factory
            .open_for_sync(&self.settings, intent, stop)
            .map_err(|f| f.map(|e| within(Section::Output, e)))
    }
    fn set_power(&self, power: Power, stop: &Hangup) -> Result<()> {
        self.factory
            .set_power(&self.settings, power, stop)
            .map_err(|e| within(Section::Output, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Strict;

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StrictSettings {
        fps: u32,
    }

    impl SourceFactory for Strict {
        const KIND: &'static str = "strict";
        type Settings = StrictSettings;
        fn validate(&self, _: &StrictSettings) -> Result<(), ValidationError> {
            Ok(())
        }
        fn open(&self, _: &StrictSettings) -> Result<Box<dyn FrameSource>> {
            Err(Error::other("never opened"))
        }
    }

    /// Where decoding stopped, under the section: the section alone when
    /// the section as a whole is at fault.
    #[test]
    fn candidates_omit_optional_details_and_round_trip_adapter_fields() {
        let fields = serde_json::json!({ "device": "/dev/video0" });
        let bare = Candidate::new("USB", &fields);
        let detailed = bare.clone().detail("screen").led_count(120);
        for (candidate, expected) in [
            (
                bare,
                serde_json::json!({ "label": "USB", "fields": fields }),
            ),
            (
                detailed,
                serde_json::json!({ "label": "USB", "fields": fields, "detail": "screen", "led_count": 120 }),
            ),
        ] {
            assert_eq!(serde_json::to_value(&candidate).unwrap(), expected);
            assert_eq!(
                serde_json::from_value::<Candidate>(expected).unwrap(),
                candidate
            );
        }
    }

    #[test]
    fn undecodable_fields_are_named_under_their_section() {
        let mut registry = Registry::default();
        registry.add_source(Strict);
        let field = |fields: Value| {
            let Value::Object(fields) = fields else {
                panic!("an object");
            };
            let capture = AdapterSettings {
                kind: Strict::KIND.into(),
                fields,
            };
            let settings = Settings {
                capture: Some(capture.clone()),
                ..Settings::default()
            };
            let validated = registry.validate(&settings).unwrap_err();
            let modes = registry.capture_modes(&capture).unwrap_err();
            let opened = registry.source(&capture).err().unwrap();
            [validated, modes, opened].map(|error| match error {
                Error::Decode { kind, field, .. } if kind == Strict::KIND => field,
                other => panic!("not a decode error: {other:?}"),
            })
        };
        assert_eq!(field(json!({ "fps": "fast" })), ["capture.fps"; 3]);
        assert_eq!(field(json!({ "fps": 30, "size": 1 })), ["capture.size"; 3]);
        assert_eq!(field(json!({})), ["capture"; 3]);
    }
}
