//! Settings persisted as one pretty-printed JSON file. See README.md for guarantees.
pub mod atomic;

use atomic::Staged;
use prysm_ports::{Error, Result, Settings, SettingsStore};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{DeserializeSeed, IgnoredAny, IntoDeserializer, MapAccess, Visitor, value},
};
use serde_json::{Number, Value};
use std::{
    ffi::OsStr,
    fmt, fs, io,
    io::Read,
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
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormatError {
    /// No `format` field: written by hand or by a Prysm before 1.0.
    #[error(
        "it has no `{FORMAT_FIELD}` field: add `\"{FORMAT_FIELD}\": {FORMAT}` \
         once its fields match this Prysm's settings"
    )]
    Missing,
    /// `format` is not a whole number of zero or more (`null`, `"1"`, `2.0`,
    /// `-1`); `found` is its JSON.
    #[error(
        "its `{FORMAT_FIELD}` is {found}, not a format number; this Prysm reads format {FORMAT}"
    )]
    NotANumber { found: String },
    /// Written by a newer Prysm: a whole number above [`FORMAT`], of any size.
    #[error("it is format {found}, written by a newer Prysm; this Prysm reads format {FORMAT}")]
    Newer { found: Number },
    /// A format this Prysm does not read: 0, or one it replaced.
    #[error("it is format {found}; this Prysm reads format {FORMAT}")]
    Older { found: Number },
}

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
        <Settings as Deserialize>::deserialize(value::MapAccessDeserializer::new(WithoutFormat(
            map,
        )))
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

/// Why bytes are not settings this Prysm reads.
enum Refused {
    TooLarge(io::Error),
    Json(serde_json::Error),
    Format(FormatError),
}

/// The settings in a file's bytes. `load` reads the file with it and `save`
/// checks what it writes with it, so whatever is saved loads.
fn decode(bytes: &[u8]) -> std::result::Result<Settings, Refused> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Refused::TooLarge(io::Error::new(
            io::ErrorKind::FileTooLarge,
            "it is larger than 1 MiB",
        )));
    }
    let file: Value = serde_json::from_slice(bytes).map_err(Refused::Json)?;
    check_format(&file).map_err(Refused::Format)?;
    settings(bytes).map_err(Refused::Json)
}

/// What a file the last `load` could not read is renamed to at the next
/// `save`: its own name, this, and six random characters, in its directory.
pub const UNREADABLE: &str = ".unreadable-";

/// A [`SettingsStore`] backed by one JSON file, replaced atomically on save.
/// A file that the last `load` could not read is never replaced: the next
/// `save` first moves it beside the new one (see [`UNREADABLE`]).
#[derive(Debug, Clone)]
pub struct SettingsFile {
    path: PathBuf,
    /// The last `load` found something at the path that it could not read.
    unread: bool,
    sync_directory: fn(&Path) -> io::Result<()>,
    /// Renames the written temporary file over the path.
    replace: fn(Staged, &Path) -> io::Result<()>,
    /// Gives the file a second name.
    link: fn(&Path, &Path) -> io::Result<()>,
}

impl SettingsFile {
    /// Nothing is touched until the first `load` or `save`. A relative path
    /// resolves against the working directory at each call.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            unread: false,
            sync_directory: atomic::sync_directory,
            replace: Staged::replace,
            link: |from, to| fs::hard_link(from, to),
        }
    }

    #[cfg(test)]
    fn with_link(mut self, link: fn(&Path, &Path) -> io::Result<()>) -> Self {
        self.link = link;
        self
    }

    #[cfg(test)]
    fn with_replace(mut self, replace: fn(Staged, &Path) -> io::Result<()>) -> Self {
        self.replace = replace;
        self
    }

    #[cfg(test)]
    fn with_directory_sync(mut self, sync: fn(&Path) -> io::Result<()>) -> Self {
        self.sync_directory = sync;
        self
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn directory(&self) -> &Path {
        atomic::directory(&self.path)
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

    fn read(&self) -> Result<Option<Settings>> {
        let file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(self.failed("read", error)),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| self.failed("read", error))?;
        decode(&bytes).map(Some).map_err(|refused| match refused {
            Refused::TooLarge(error) => self.failed("read", error),
            Refused::Json(error) => self.failed("parse", error),
            Refused::Format(error) => self.failed("load", error),
        })
    }

    /// Keep the file at the path under a name of its own beside it; `None`
    /// when nothing is there any more. In order of preference:
    ///
    /// - a hard link: the file stays at its path until the save's rename
    ///   replaces it there;
    /// - where a hard link fails (no hard links on the filesystem; on Linux
    ///   `fs.protected_hardlinks` for a file this user neither owns nor may
    ///   write), a copy with its permissions, which needs the file readable;
    ///   the file stays at its path likewise;
    /// - where both fail, a rename: until the save's rename the path holds
    ///   nothing, and a crash in between leaves it so (the next start is
    ///   then unconfigured, with no failure shown), the file kept beside.
    fn set_aside(&self) -> std::result::Result<Option<Aside>, NotSetAside> {
        let mut prefix = self
            .path
            .file_name()
            .unwrap_or(OsStr::new("settings"))
            .to_os_string();
        prefix.push(UNREADABLE);
        let mut builder = tempfile::Builder::new();
        builder.prefix(&prefix);
        let directory = self.directory();
        // A name nothing has: a hard link or a new file never replaces one.
        let link = match builder.make_in(directory, |aside| (self.link)(&self.path, aside)) {
            Ok(aside) => return Ok(Some(Aside::Linked(aside.into_temp_path()))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(link) => link,
        };
        let copy = match builder.make_in(directory, |aside| copy_new(&self.path, aside)) {
            Ok(aside) => return Ok(Some(Aside::Linked(aside.into_temp_path()))),
            Err(copy) => copy,
        };
        // A new file of its own, so the rename replaces nothing else.
        let moved = builder
            .tempfile_in(directory)
            .map(tempfile::NamedTempFile::into_temp_path)
            .and_then(|placeholder| {
                fs::rename(&self.path, &placeholder)?;
                placeholder.keep().map_err(|error| error.error)
            });
        match moved {
            Ok(aside) => Ok(Some(Aside::Moved(aside))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(rename) => Err(NotSetAside { link, copy, rename }),
        }
    }
}

/// Where the file a load could not read is kept.
enum Aside {
    /// A second name: the file is still at its path. Removed when dropped,
    /// unless kept.
    Linked(tempfile::TempPath),
    /// Its only name: the path holds nothing until the save's rename.
    Moved(PathBuf),
}

/// A save whose rename failed after the file a load could not read was
/// moved aside, and that could not be moved back: the path now holds no
/// file, and the old one is only at `aside`.
#[derive(Debug, thiserror::Error)]
#[error(
    "{replace}; the file it could not read is now only at {}: moving it back failed: {back}",
    aside.display()
)]
pub struct LeftAside {
    replace: io::Error,
    aside: PathBuf,
    back: io::Error,
}

/// Why the file a load could not read could not be kept: each way's cause.
#[derive(Debug, thiserror::Error)]
#[error("a hard link failed: {link}; a copy failed: {copy}; a rename failed: {rename}")]
pub struct NotSetAside {
    link: io::Error,
    copy: io::Error,
    rename: io::Error,
}

impl SettingsStore for SettingsFile {
    fn load_mode(&mut self) -> Result<Option<prysm_ports::LightingMode>> {
        let path = self.path.with_added_extension("mode");
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(Error::adapter(format!("open {}", path.display()), error)),
        };
        serde_json::from_reader(io::BufReader::new(file))
            .map(Some)
            .map_err(|error| Error::adapter(format!("read {}", path.display()), error))
    }

    fn save_mode(&mut self, mode: prysm_ports::LightingMode) -> Result<()> {
        let path = self.path.with_added_extension("mode");
        let bytes = serde_json::to_vec(&mode)
            .map_err(|error| Error::adapter("encode remembered mode", error))?;
        atomic::write(&path, &bytes, None, true)
            .map_err(|error| Error::adapter(format!("save {}", path.display()), error))
    }

    fn load(&mut self) -> Result<Option<Settings>> {
        let loaded = self.read();
        self.unread = loaded.is_err();
        loaded
    }

    /// `Err` leaves the stored settings as they were; `Ok` once the new file
    /// is in place, even if syncing its directory then fails (logged).
    fn save(&mut self, settings: &Settings) -> Result<()> {
        let saved = Saved {
            format: FORMAT,
            settings,
        };
        let mut bytes =
            serde_json::to_vec_pretty(&saved).map_err(|error| self.failed("encode", error))?;
        bytes.push(b'\n');
        decode(&bytes).map_err(|refused| match refused {
            Refused::TooLarge(error) => self.failed("save", error),
            Refused::Json(error) => self.failed("save", error),
            Refused::Format(error) => self.failed("save", error),
        })?;

        // Everything before the rename only touches the temporary file, which
        // is removed on drop; the existing file stays intact.
        let mut temporary = Staged::new(&self.path)
            .map_err(|error| self.failed("create a temporary file for", error))?;
        match fs::metadata(&self.path) {
            Ok(existing) if !existing.is_file() => {
                return Err(self.failed(
                    "replace",
                    io::Error::new(io::ErrorKind::InvalidInput, "it is not a regular file"),
                ));
            }
            // The file set aside keeps its permissions; the new one has the
            // temporary file's, owner-only.
            Ok(_) if self.unread => {}
            Ok(existing) => temporary
                .set_permissions(existing.permissions())
                .map_err(|error| self.failed("copy permissions of", error))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(self.failed("inspect", error)),
        }
        temporary
            .write(&bytes)
            .map_err(|error| self.failed("write a temporary file for", error))?;

        let aside = if self.unread {
            self.set_aside()
                .map_err(|error| self.failed("set aside the unreadable", error))?
        } else {
            None
        };
        // The rename is the commit point. A failure leaves the unreadable
        // file at its path: its second name is removed, or a file moved
        // aside goes back.
        if let Err(replace) = (self.replace)(temporary, &self.path) {
            if let Some(Aside::Moved(aside)) = aside
                && let Err(back) = fs::rename(&aside, &self.path)
            {
                return Err(self.failed(
                    "replace",
                    LeftAside {
                        replace,
                        aside,
                        back,
                    },
                ));
            }
            return Err(self.failed("replace", replace));
        }
        self.unread = false;
        match aside {
            None => {}
            Some(Aside::Moved(aside)) => tracing::warn!(
                "kept the settings file it could not read as {}",
                aside.display()
            ),
            // Saved: a name that cannot be kept is logged, not a failure.
            Some(Aside::Linked(aside)) => match aside.keep() {
                Ok(aside) => tracing::warn!(
                    "kept the settings file it could not read as {}",
                    aside.display()
                ),
                Err(error) => tracing::warn!(
                    "saved settings file {}, but could not keep the file it could not \
                     read beside it: {}",
                    self.path.display(),
                    error.error
                ),
            },
        }
        if let Err(error) = (self.sync_directory)(self.directory()) {
            tracing::warn!(
                "saved settings file {}, but syncing its directory failed, so the \
                 save may not survive a crash: {error}",
                self.path.display()
            );
        }
        Ok(())
    }
}

/// Copy `from` to the new file `to`, which must not exist; nothing is left
/// at `to` when it fails.
fn copy_new(from: &Path, to: &Path) -> io::Result<()> {
    let mut target = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)?;
    let copied = fs::File::open(from)
        .and_then(|mut source| {
            io::copy(&mut source, &mut target)?;
            target.set_permissions(source.metadata()?.permissions())
        })
        .and_then(|()| target.sync_all());
    if copied.is_err() {
        let _ = fs::remove_file(to);
    }
    copied
}

#[cfg(test)]
mod tests;
