use crate::{
    AdapterSettings, Error, FrameSource, LedOutput, Result, Section, Settings, ValidationError,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
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
            find(&self.sources, section, capture)?
                .validate(capture)
                .map_err(|error| within(section, error))?;
        }
        if let Some(output) = &settings.output {
            let section = Section::Output;
            find(&self.outputs, section, output)?
                .validate(output)
                .map_err(|error| within(section, error))?;
        }
        Ok(())
    }

    /// # Errors
    /// Unknown kind, invalid settings or the factory's open failure.
    pub fn open_source(&self, settings: &AdapterSettings) -> Result<Box<dyn FrameSource>> {
        let section = Section::Capture;
        find(&self.sources, section, settings)?
            .open(settings)
            .map_err(|error| within(section, error))
    }

    /// # Errors
    /// Unknown kind, invalid settings or the factory's open failure.
    pub fn open_output(&self, settings: &AdapterSettings) -> Result<Box<dyn LedOutput>> {
        let section = Section::Output;
        find(&self.outputs, section, settings)?
            .open(settings)
            .map_err(|error| within(section, error))
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
    settings: &AdapterSettings,
) -> Result<&'a dyn Erased<T>> {
    map.get(settings.kind.as_str())
        .map(|factory| &**factory)
        .ok_or_else(|| {
            ValidationError::new(
                format!("{section}.kind"),
                format!("unknown kind `{}`", settings.kind),
            )
            .into()
        })
}

trait Erased<T: ?Sized>: Send + Sync {
    fn validate(&self, settings: &AdapterSettings) -> Result<()>;
    fn open(&self, settings: &AdapterSettings) -> Result<Box<T>>;
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
}
