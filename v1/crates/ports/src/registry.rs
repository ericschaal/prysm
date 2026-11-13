use crate::{
    AdapterSettings, Error, FrameSource, LedOutput, Result, Section, Settings, ValidationError,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Opens a [`FrameSource`] from typed settings. Opening may block.
pub trait SourceFactory: Send + Sync + 'static {
    /// Tag in the settings file, such as `"device"` or `"webos"`.
    const KIND: &'static str;
    type Settings: DeserializeOwned + Serialize;
    /// # Errors
    /// Rejects settings that can never open, without touching hardware. The
    /// field is relative to the section, such as `fps`: the registry names
    /// the section.
    fn validate(&self, settings: &Self::Settings) -> Result<(), ValidationError>;
    /// # Errors
    /// Device or I/O failure.
    fn open(&self, settings: &Self::Settings) -> Result<Box<dyn FrameSource>>;
    /// What this kind finds to choose from, such as the video devices present.
    /// See [`Candidate`]; the default finds nothing.
    fn candidates(&self) -> Vec<Candidate> {
        Vec::new()
    }
}

/// Opens a [`LedOutput`] from typed settings. Opening may block.
pub trait OutputFactory: Send + Sync + 'static {
    const KIND: &'static str;
    type Settings: DeserializeOwned + Serialize;
    /// # Errors
    /// Rejects settings that can never open, without touching hardware. The
    /// field is relative to the section, such as `address`: the registry
    /// names the section.
    fn validate(&self, settings: &Self::Settings) -> Result<(), ValidationError>;
    /// # Errors
    /// Device or I/O failure.
    fn open(&self, settings: &Self::Settings) -> Result<Box<dyn LedOutput>>;
    /// What this kind finds to choose from, such as devices on the network.
    /// See [`Candidate`]; the default finds nothing.
    fn candidates(&self) -> Vec<Candidate> {
        Vec::new()
    }
}

/// Something a factory found that a person can pick instead of typing its
/// settings: a device present, a device answering on the network.
///
/// Finding is an on-request lookup, not state: it blocks, ends within about
/// two seconds, opens nothing and keeps nothing (no threads, caches or
/// polling). A factory that cannot look returns nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// What a person recognises, such as the device's name.
    pub label: String,
    /// Where it is or what it has, such as `192.168.1.40, 120 LEDs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The settings fields picking it fills, such as `{ "path": "/dev/video0" }`:
    /// part of the kind's settings, merged over what the person has.
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
            fields,
        }
    }

    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// The adapters a host offers, keyed by kind. Erases factory types so the
/// engine and the settings file need no generics.
#[derive(Clone, Default)]
pub struct Registry {
    sources: BTreeMap<&'static str, Arc<dyn Erased<dyn FrameSource>>>,
    outputs: BTreeMap<&'static str, Arc<dyn Erased<dyn LedOutput>>>,
}

impl Registry {
    /// # Panics
    /// If a source of the same kind is already registered.
    pub fn add_source<F: SourceFactory>(&mut self, factory: F) {
        let previous = self.sources.insert(F::KIND, Arc::new(Source(factory)));
        assert!(previous.is_none(), "duplicate source kind `{}`", F::KIND);
    }

    /// # Panics
    /// If an output of the same kind is already registered.
    pub fn add_output<F: OutputFactory>(&mut self, factory: F) {
        let previous = self.outputs.insert(F::KIND, Arc::new(Output(factory)));
        assert!(previous.is_none(), "duplicate output kind `{}`", F::KIND);
    }

    /// Validate processing (including color profiles and the active profile)
    /// and every configured adapter against its factory.
    ///
    /// # Errors
    /// [`Error::Invalid`] or [`Error::Decode`].
    pub fn validate(&self, settings: &Settings) -> Result<()> {
        settings.processing.validate()?;
        if let Some(capture) = &settings.capture {
            let section = Section::Capture;
            find(&self.sources, section, &capture.kind)?
                .validate(capture)
                .map_err(|error| within(section, error))?;
        }
        if let Some(output) = &settings.output {
            let section = Section::Output;
            find(&self.outputs, section, &output.kind)?
                .validate(output)
                .map_err(|error| within(section, error))?;
        }
        Ok(())
    }

    /// # Errors
    /// Unknown kind, invalid settings or the factory's open failure.
    pub fn open_source(&self, settings: &AdapterSettings) -> Result<Box<dyn FrameSource>> {
        let section = Section::Capture;
        find(&self.sources, section, &settings.kind)?
            .open(settings)
            .map_err(|error| within(section, error))
    }

    /// # Errors
    /// Unknown kind, invalid settings or the factory's open failure.
    pub fn open_output(&self, settings: &AdapterSettings) -> Result<Box<dyn LedOutput>> {
        let section = Section::Output;
        find(&self.outputs, section, &settings.kind)?
            .open(settings)
            .map_err(|error| within(section, error))
    }

    /// What the factory registered for `kind` in `section` finds to choose
    /// from. Blocking, for up to about two seconds; see [`Candidate`].
    ///
    /// # Errors
    /// [`Error::Invalid`] (field `<section>.kind`) for a kind not registered
    /// in `section`, or for the processing section, which has no kinds.
    pub fn candidates(&self, section: Section, kind: &str) -> Result<Vec<Candidate>> {
        Ok(match section {
            Section::Capture => find(&self.sources, section, kind)?.candidates(),
            Section::Output => find(&self.outputs, section, kind)?.candidates(),
            Section::Processing => {
                return Err(ValidationError::new("processing", "has no adapter kinds").into());
            }
        })
    }
}

/// A factory's invalid field, relative to its section, as a full path.
fn within(section: Section, error: Error) -> Error {
    match error {
        Error::Invalid(ValidationError { field, reason }) => Error::Invalid(ValidationError {
            field: format!("{section}.{field}"),
            reason,
        }),
        other => other,
    }
}

fn find<'a, T: ?Sized>(
    map: &'a BTreeMap<&'static str, Arc<dyn Erased<T>>>,
    section: Section,
    kind: &str,
) -> Result<&'a dyn Erased<T>> {
    map.get(kind).map(|factory| &**factory).ok_or_else(|| {
        ValidationError::new(format!("{section}.kind"), format!("unknown kind `{kind}`")).into()
    })
}

trait Erased<T: ?Sized>: Send + Sync {
    fn validate(&self, settings: &AdapterSettings) -> Result<()>;
    fn open(&self, settings: &AdapterSettings) -> Result<Box<T>>;
    fn candidates(&self) -> Vec<Candidate>;
}

fn decode<S: DeserializeOwned>(settings: &AdapterSettings) -> Result<S> {
    serde_json::from_value(Value::Object(settings.fields.clone())).map_err(|source| Error::Decode {
        kind: settings.kind.clone(),
        source: Arc::new(source),
    })
}

struct Source<F>(F);
struct Output<F>(F);

impl<F: SourceFactory> Erased<dyn FrameSource> for Source<F> {
    fn validate(&self, settings: &AdapterSettings) -> Result<()> {
        Ok(self.0.validate(&decode(settings)?)?)
    }
    fn open(&self, settings: &AdapterSettings) -> Result<Box<dyn FrameSource>> {
        let typed = decode(settings)?;
        self.0.validate(&typed)?;
        self.0.open(&typed)
    }
    fn candidates(&self) -> Vec<Candidate> {
        self.0.candidates()
    }
}

impl<F: OutputFactory> Erased<dyn LedOutput> for Output<F> {
    fn validate(&self, settings: &AdapterSettings) -> Result<()> {
        Ok(self.0.validate(&decode(settings)?)?)
    }
    fn open(&self, settings: &AdapterSettings) -> Result<Box<dyn LedOutput>> {
        let typed = decode(settings)?;
        self.0.validate(&typed)?;
        self.0.open(&typed)
    }
    fn candidates(&self) -> Vec<Candidate> {
        self.0.candidates()
    }
}
