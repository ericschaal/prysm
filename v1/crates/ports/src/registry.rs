use crate::{AdapterSettings, Error, FrameSource, LedOutput, Result, Settings, ValidationError};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

// Schema export hook: when the OpenAPI export moves here, add a `ToSchema` bound
// on `Settings` behind a feature and collect schemas per kind in `Registry`.

/// Opens a [`FrameSource`] from typed settings. Opening may block.
pub trait SourceFactory: Send + Sync + 'static {
    /// Tag in the settings file, such as `"device"` or `"webos"`.
    const KIND: &'static str;
    type Settings: DeserializeOwned + Serialize;
    /// # Errors
    /// Rejects settings that can never open, without touching hardware.
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
    /// Rejects settings that can never open, without touching hardware.
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

    pub fn source_kinds(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.sources.keys().copied()
    }

    pub fn output_kinds(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.outputs.keys().copied()
    }

    /// Validate processing and every configured adapter against its factory.
    ///
    /// # Errors
    /// [`Error::Invalid`] or [`Error::Decode`].
    pub fn validate(&self, settings: &Settings) -> Result<()> {
        settings.processing.validate()?;
        if let Some(capture) = &settings.capture {
            find(&self.sources, "capture", capture)?.validate(capture)?;
        }
        if let Some(output) = &settings.output {
            find(&self.outputs, "output", output)?.validate(output)?;
        }
        Ok(())
    }

    /// # Errors
    /// Unknown kind, invalid settings or the factory's open failure.
    pub fn open_source(&self, settings: &AdapterSettings) -> Result<Box<dyn FrameSource>> {
        find(&self.sources, "capture", settings)?.open(settings)
    }

    /// # Errors
    /// Unknown kind, invalid settings or the factory's open failure.
    pub fn open_output(&self, settings: &AdapterSettings) -> Result<Box<dyn LedOutput>> {
        find(&self.outputs, "output", settings)?.open(settings)
    }
}

fn find<'a, T: ?Sized>(
    map: &'a BTreeMap<&'static str, Arc<dyn Erased<T>>>,
    section: &str,
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
