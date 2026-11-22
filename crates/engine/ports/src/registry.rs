use crate::{
    AdapterSettings, Error, FrameSource, IdleMode, LedOutput, Modes, Result, Section, Settings,
    ValidationError,
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
    type Settings: DeserializeOwned + Serialize;
    /// # Errors
    /// Rejects settings that can never open, without touching hardware. The
    /// field is relative to the section, such as `fps`: the registry names
    /// the section.
    fn validate(&self, settings: &Self::Settings) -> Result<(), ValidationError>;
    /// # Errors
    /// Device or I/O failure, or a size or rate [`Self::modes`] says the
    /// hardware does not deliver.
    fn open(&self, settings: &Self::Settings) -> Result<Box<dyn FrameSource>>;
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
    type Settings: DeserializeOwned + Serialize;
    /// # Errors
    /// Rejects settings that can never open, without touching hardware. The
    /// field is relative to the section, such as `address`: the registry
    /// names the section.
    fn validate(&self, settings: &Self::Settings) -> Result<(), ValidationError>;
    /// # Errors
    /// Device or I/O failure, saying whether the device may have taken a
    /// request on the way ([`OpenFailure`]).
    fn open(&self, settings: &Self::Settings) -> Result<Box<dyn LedOutput>, OpenFailure>;
    /// Put the device in `mode` without taking it over: what opening it and
    /// idling at once leaves, for a device nobody holds (lights turned off
    /// after a released session, or the device's own effects without one).
    /// The default does just that; a factory that can set the mode alone
    /// does so, so the device never shows Prysm's colours meanwhile.
    ///
    /// # Errors
    /// Device or I/O failure.
    fn idle(&self, settings: &Self::Settings, mode: IdleMode) -> Result<()> {
        self.open(settings)
            .map_err(OpenFailure::into_error)?
            .idle(mode)
    }
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
    /// Where it is or what it has, such as `192.168.1.40`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// How many LEDs a found output device says it has, when it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led_count: Option<u32>,
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
        let previous = self.sources.insert(F::KIND, Arc::new(Source(factory)));
        assert!(previous.is_none(), "duplicate source kind `{}`", F::KIND);
    }

    /// # Panics
    /// If an output of the same kind is already registered.
    pub fn add_output<F: OutputFactory>(&mut self, factory: F) {
        let previous = self.outputs.insert(F::KIND, Arc::new(Output(factory)));
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
    /// Unknown kind or invalid settings (the device untouched), or the
    /// factory's open failure.
    pub fn open_output(
        &self,
        settings: &AdapterSettings,
    ) -> Result<Box<dyn LedOutput>, OpenFailure> {
        let section = Section::Output;
        find(&self.outputs, section, &settings.kind)
            .map_err(OpenFailure::Untouched)?
            .open(settings)
            .map_err(|failure| failure.map(|error| within(section, error)))
    }

    /// Put the configured output in `mode` without opening it; see
    /// [`OutputFactory::idle`].
    ///
    /// # Errors
    /// Unknown kind, invalid settings or the factory's failure.
    pub fn idle_output(&self, settings: &AdapterSettings, mode: IdleMode) -> Result<()> {
        let section = Section::Output;
        find(&self.outputs, section, &settings.kind)?
            .idle(settings, mode)
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

    /// What the capture `settings` select delivers; see
    /// [`SourceFactory::modes`]. Blocking, briefly.
    ///
    /// # Errors
    /// Unknown kind (field `capture.kind`) or undecodable settings.
    pub fn capture_modes(&self, settings: &AdapterSettings) -> Result<Modes> {
        find(&self.sources, Section::Capture, &settings.kind)?.modes(settings)
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

fn find<'a, F: ?Sized>(
    map: &'a BTreeMap<&'static str, Arc<F>>,
    section: Section,
    kind: &str,
) -> Result<&'a F> {
    map.get(kind).map(|factory| &**factory).ok_or_else(|| {
        ValidationError::new(format!("{section}.kind"), format!("unknown kind `{kind}`")).into()
    })
}

trait Erased<T: ?Sized, E>: Send + Sync {
    fn validate(&self, settings: &AdapterSettings) -> Result<()>;
    fn open(&self, settings: &AdapterSettings) -> Result<Box<T>, E>;
    fn candidates(&self) -> Vec<Candidate>;
}

trait ErasedSource: Erased<dyn FrameSource, Error> {
    fn modes(&self, settings: &AdapterSettings) -> Result<Modes>;
}

trait ErasedOutput: Erased<dyn LedOutput, OpenFailure> {
    fn idle(&self, settings: &AdapterSettings, mode: IdleMode) -> Result<()>;
}

fn decode<S: DeserializeOwned>(settings: &AdapterSettings) -> Result<S> {
    serde_json::from_value(Value::Object(settings.fields.clone())).map_err(|source| Error::Decode {
        kind: settings.kind.clone(),
        source: Arc::new(source),
    })
}

struct Source<F>(F);
struct Output<F>(F);

impl<F: SourceFactory> Erased<dyn FrameSource, Error> for Source<F> {
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

impl<F: SourceFactory> ErasedSource for Source<F> {
    fn modes(&self, settings: &AdapterSettings) -> Result<Modes> {
        Ok(self.0.modes(&decode(settings)?))
    }
}

impl<F: OutputFactory> ErasedOutput for Output<F> {
    fn idle(&self, settings: &AdapterSettings, mode: IdleMode) -> Result<()> {
        let typed = decode(settings)?;
        self.0.validate(&typed)?;
        self.0.idle(&typed, mode)
    }
}

impl<F: OutputFactory> Erased<dyn LedOutput, OpenFailure> for Output<F> {
    fn validate(&self, settings: &AdapterSettings) -> Result<()> {
        Ok(self.0.validate(&decode(settings)?)?)
    }
    fn open(&self, settings: &AdapterSettings) -> Result<Box<dyn LedOutput>, OpenFailure> {
        let typed = decode(settings).map_err(OpenFailure::Untouched)?;
        self.0
            .validate(&typed)
            .map_err(|invalid| OpenFailure::Untouched(invalid.into()))?;
        self.0.open(&typed)
    }
    fn candidates(&self) -> Vec<Candidate> {
        self.0.candidates()
    }
}
