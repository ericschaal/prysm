//! Settings persisted as one pretty-printed JSON file. See README.md for guarantees.
use prysm_ports::{Error, Result, Settings, SettingsStore};
use std::{
    fs, io,
    io::Write,
    path::{Path, PathBuf},
};

/// A [`SettingsStore`] backed by one JSON file, replaced atomically on save.
#[derive(Debug, Clone)]
pub struct FileSettings {
    path: PathBuf,
}

impl FileSettings {
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

impl SettingsStore for FileSettings {
    fn load(&mut self) -> Result<Option<Settings>> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(self.failed("read", error)),
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| self.failed("parse", error))
    }

    fn save(&mut self, settings: &Settings) -> Result<()> {
        let mut bytes =
            serde_json::to_vec_pretty(settings).map_err(|error| self.failed("encode", error))?;
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
