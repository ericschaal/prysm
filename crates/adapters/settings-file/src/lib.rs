//! Settings persisted as one pretty-printed JSON file. See README.md for guarantees.
pub mod atomic;

use atomic::Staged;
use prysm_ports::{AdapterSettings, Error, Processing, Result, Settings, SettingsStore};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use std::{
    ffi::OsStr,
    fs, io,
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
    let format = object.get(FORMAT_FIELD).ok_or(FormatError::Missing)?;
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

#[derive(Deserialize, Serialize)]
#[serde(remote = "Self", deny_unknown_fields)]
struct FileSettings {
    format: u32,
    capture: Option<AdapterSettings>,
    output: Option<AdapterSettings>,
    #[serde(default)]
    processing: Processing,
}
prysm_ports::map_only!(FileSettings, "a settings object");

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
    let file: FileSettings = serde_json::from_slice(bytes).map_err(Refused::Json)?;
    Ok(Settings {
        capture: file.capture,
        output: file.output,
        processing: file.processing,
    })
}

/// Suffix before the six random characters in an unreadable file's backup name.
pub const UNREADABLE: &str = ".unreadable-";

/// A [`SettingsStore`] backed by one JSON file, replaced atomically on save.
/// After a failed load, saving requires a backup of the original file (see [`UNREADABLE`]).
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

    /// Returns the retained backup path, or `None` if the original has disappeared.
    fn backup(&self) -> Result<Option<PathBuf>> {
        let mut prefix = self
            .path
            .file_name()
            .unwrap_or(OsStr::new("settings"))
            .to_os_string();
        prefix.push(UNREADABLE);
        let mut builder = tempfile::Builder::new();
        builder.prefix(&prefix);
        let backup = builder
            .make_in(self.directory(), |path| (self.link)(&self.path, path))
            .map(tempfile::NamedTempFile::into_temp_path)
            .or_else(|link| {
                copy_aside(&self.path, &builder).map_err(|copy| NotSetAside { link, copy })
            });
        match backup {
            Ok(path) => path
                .keep()
                .map(Some)
                .map_err(|e| self.failed("keep a backup of", e.error)),
            Err(failure) => match self.path.symlink_metadata() {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                _ => Err(self.failed("back up unreadable", failure)),
            },
        }
    }
}

/// Both ways of preserving an unreadable settings file failed.
#[derive(Debug, thiserror::Error)]
#[error(
    "hard link failed ({link}) and copy failed ({copy}). Check the file permissions or move the file, then save again."
)]
pub struct NotSetAside {
    link: io::Error,
    copy: io::Error,
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

        let backup = if self.unread { self.backup()? } else { None };
        let published = if self.unread && backup.is_none() {
            temporary.create(&self.path)
        } else {
            (self.replace)(temporary, &self.path)
        };
        if let Err(error) = published {
            if let Some(path) = backup
                && let Err(cleanup) = fs::remove_file(&path)
            {
                tracing::warn!(
                    "could not remove unused settings backup {}: {cleanup}",
                    path.display()
                );
            }
            return Err(self.failed("replace", error));
        }
        self.unread = false;
        if let Some(path) = backup {
            tracing::warn!("kept the unreadable settings file as {}", path.display());
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

fn copy_aside(path: &Path, builder: &tempfile::Builder<'_, '_>) -> io::Result<tempfile::TempPath> {
    let mut source = fs::File::open(path)?;
    let mut copy = builder.tempfile_in(atomic::directory(path))?;
    io::copy(&mut source, copy.as_file_mut())?;
    copy.as_file()
        .set_permissions(source.metadata()?.permissions())?;
    copy.as_file().sync_all()?;
    Ok(copy.into_temp_path())
}

#[cfg(test)]
mod tests;
