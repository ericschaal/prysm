//! Settings persisted as one pretty-printed JSON file. See README.md for guarantees.
use prysm_ports::{Error, Result, Settings, SettingsStore};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{DeserializeSeed, IgnoredAny, IntoDeserializer, MapAccess, Visitor, value},
};
use serde_json::{Number, Value};
use std::{
    fmt, fs, io,
    io::{Read, Write},
    path::{Path, PathBuf},
};

/// The file format this Prysm writes and reads, saved as the file's `format`
/// field. Raise it only when this Prysm can no longer read files with the
/// previous number: a field renamed or removed, or its meaning changed.
/// Additions keep it: an older Prysm refuses a newer file by the unknown
/// field's name. A file with another number is refused; there is no
/// migration (see docs/developers/releasing.md).
pub const FORMAT: u32 = 1;

const FORMAT_FIELD: &str = "format";

/// Larger files are refused before parsing: a wrong path (a device, a log)
/// must not be read whole.
const MAX_BYTES: u64 = 1024 * 1024;

/// A settings file whose `format` this Prysm does not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// No `format` field: written by hand or by a Prysm before 1.0.
    Missing,
    /// `format` is not a whole number of zero or more (`null`, `"1"`, `2.0`,
    /// `-1`); `found` is its JSON.
    NotANumber { found: String },
    /// Written by a newer Prysm: a whole number above [`FORMAT`], of any size.
    Newer { found: Number },
    /// A format this Prysm does not read: 0, or one it replaced.
    Older { found: Number },
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(
                f,
                "it has no `{FORMAT_FIELD}` field: add `\"{FORMAT_FIELD}\": {FORMAT}` \
                 once its fields match this Prysm's settings"
            ),
            Self::NotANumber { found } => write!(
                f,
                "its `{FORMAT_FIELD}` is {found}, not a format number; \
                 this Prysm reads format {FORMAT}"
            ),
            Self::Newer { found } => write!(
                f,
                "it is format {found}, written by a newer Prysm; this Prysm reads format {FORMAT}"
            ),
            Self::Older { found } => {
                write!(f, "it is format {found}; this Prysm reads format {FORMAT}")
            }
        }
    }
}

impl std::error::Error for FormatError {}

/// Checks the `format` of a file's object before its settings are read, so
/// that a newer file is refused for its format rather than for a field it
/// adds. Anything but an object is left to the settings' own parse.
fn check_format(file: &Value) -> std::result::Result<(), FormatError> {
    let Some(object) = file.as_object() else {
        return Ok(());
    };
    let Some(format) = object.get(FORMAT_FIELD) else {
        return Err(FormatError::Missing);
    };
    let not_a_number = || FormatError::NotANumber {
        found: format.to_string(),
    };
    let Value::Number(found) = format else {
        return Err(not_a_number());
    };
    let current = u64::from(FORMAT);
    match found.as_u64() {
        Some(number) if number == current => Ok(()),
        Some(number) if number > current => Err(FormatError::Newer {
            found: found.clone(),
        }),
        Some(_) => Err(FormatError::Older {
            found: found.clone(),
        }),
        // Whole numbers beyond u64 arrive as floats; `2.0` was written as one.
        None if found
            .as_f64()
            .is_some_and(|number| number.fract() == 0.0 && number >= TWO_TO_THE_64) =>
        {
            Err(FormatError::Newer {
                found: found.clone(),
            })
        }
        None => Err(not_a_number()),
    }
}

/// The least whole number a `u64` cannot hold.
const TWO_TO_THE_64: f64 = 18_446_744_073_709_551_616.0;

/// The file as saved: `format` first, then the settings' fields.
#[derive(Serialize)]
struct Saved<'a> {
    format: u32,
    #[serde(flatten)]
    settings: &'a Settings,
}

/// The settings of a file whose header was checked. Serde's `flatten` would
/// let unknown fields through, so the file's object is read as the settings
/// with its `format` skipped; errors keep their line.
fn settings(bytes: &[u8]) -> serde_json::Result<Settings> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let settings = deserializer.deserialize_map(SettingsObject)?;
    deserializer.end()?;
    Ok(settings)
}

struct SettingsObject;

impl<'de> Visitor<'de> for SettingsObject {
    type Value = Settings;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a settings object")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> std::result::Result<Settings, A::Error> {
        Settings::deserialize(value::MapAccessDeserializer::new(WithoutFormat(map)))
    }
}

/// A file object's entries but `format`.
struct WithoutFormat<A>(A);

impl<'de, A: MapAccess<'de>> MapAccess<'de> for WithoutFormat<A> {
    type Error = A::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> std::result::Result<Option<K::Value>, A::Error> {
        while let Some(key) = self.0.next_key::<String>()? {
            if key != FORMAT_FIELD {
                return seed.deserialize(key.into_deserializer()).map(Some);
            }
            self.0.next_value::<IgnoredAny>()?;
        }
        Ok(None)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> std::result::Result<V::Value, A::Error> {
        self.0.next_value_seed(seed)
    }
}

/// A [`SettingsStore`] backed by one JSON file, replaced atomically on save.
#[derive(Debug, Clone)]
pub struct SettingsFile {
    path: PathBuf,
}

impl SettingsFile {
    /// Nothing is touched until the first `load` or `save`. A relative path
    /// resolves against the working directory at each call.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn directory(&self) -> &Path {
        match self.path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        }
    }

    fn failed(
        &self,
        action: &str,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Error {
        Error::adapter(
            format!("cannot {action} settings file {}", self.path.display()),
            source,
        )
    }
}

impl SettingsStore for SettingsFile {
    fn load(&mut self) -> Result<Option<Settings>> {
        let file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(self.failed("read", error)),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| self.failed("read", error))?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(self.failed(
                "read",
                io::Error::new(io::ErrorKind::FileTooLarge, "it is larger than 1 MiB"),
            ));
        }
        let file: Value =
            serde_json::from_slice(&bytes).map_err(|error| self.failed("parse", error))?;
        check_format(&file).map_err(|error| self.failed("load", error))?;
        settings(&bytes)
            .map(Some)
            .map_err(|error| self.failed("parse", error))
    }

    fn save(&mut self, settings: &Settings) -> Result<()> {
        let saved = Saved {
            format: FORMAT,
            settings,
        };
        let mut bytes =
            serde_json::to_vec_pretty(&saved).map_err(|error| self.failed("encode", error))?;
        bytes.push(b'\n');

        // Everything before the rename only touches the temporary file, which
        // is removed on drop; the existing file stays intact.
        let mut temporary = tempfile::NamedTempFile::new_in(self.directory())
            .map_err(|error| self.failed("create a temporary file for", error))?;
        match fs::metadata(&self.path) {
            Ok(existing) => temporary
                .as_file()
                .set_permissions(existing.permissions())
                .map_err(|error| self.failed("copy permissions of", error))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(self.failed("inspect", error)),
        }
        temporary
            .write_all(&bytes)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|error| self.failed("write a temporary file for", error))?;

        // The rename is the commit point.
        temporary
            .persist(&self.path)
            .map_err(|error| self.failed("replace", error.error))?;
        sync_directory(self.directory()).map_err(|error| {
            self.failed(
                "make durable (the new contents were written, but may not survive a crash)",
                error,
            )
        })
    }
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> io::Result<()> {
    fs::File::open(directory)?.sync_all()
}

/// Other platforms cannot open a directory for syncing; the rename is still atomic.
#[cfg(not(unix))]
fn sync_directory(_: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests;
