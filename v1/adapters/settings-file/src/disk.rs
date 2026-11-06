//! Same-directory atomic replacement. The rename is the commit boundary.
use prysm_contract::ControlError;
use std::{
    fs::{self, File},
    io::{self, Write},
    path::PathBuf,
};

pub(super) struct Store {
    path: PathBuf,
    #[cfg(test)]
    pub checkpoint: Option<Box<dyn FnMut(Stage) -> io::Result<()> + Send>>,
}
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    BeforeRename,
    AfterRename,
}

#[derive(Debug, thiserror::Error)]
#[error("{action} settings file {}: {source}", path.display())]
struct DiskError {
    action: &'static str,
    path: PathBuf,
    #[source]
    source: io::Error,
}

impl Store {
    pub fn new(path: PathBuf) -> Result<Self, ControlError> {
        let path =
            std::path::absolute(path).map_err(|error| ControlError::Adapter(Box::new(error)))?;
        if path.file_name().is_none() {
            return Err(ControlError::Invalid {
                field: "settings path",
                reason: "expected a file path".into(),
            });
        }
        Ok(Self {
            path,
            #[cfg(test)]
            checkpoint: None,
        })
    }
    fn failed(&self, action: &'static str, source: io::Error) -> ControlError {
        ControlError::Adapter(Box::new(DiskError {
            action,
            path: self.path.clone(),
            source,
        }))
    }
    fn uncertain_commit(&self, source: io::Error) -> ControlError {
        ControlError::RecoveryRequired(Box::new(DiskError {
            action: "confirm durability of",
            path: self.path.clone(),
            source,
        }))
    }
    pub fn load(&self) -> Result<Option<Vec<u8>>, ControlError> {
        match fs::read(&self.path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(self.failed("read", error)),
        }
    }
    #[cfg(unix)]
    pub fn save(&mut self, bytes: &[u8]) -> Result<(), ControlError> {
        let directory_path = self.path.parent().expect("absolute file path");
        let directory =
            File::open(directory_path).map_err(|error| self.failed("open directory for", error))?;
        let mut temporary = tempfile::NamedTempFile::new_in(directory_path)
            .map_err(|error| self.failed("create temporary", error))?;
        temporary
            .write_all(bytes)
            .map_err(|error| self.failed("write temporary", error))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| self.failed("sync temporary", error))?;
        #[cfg(test)]
        if let Some(checkpoint) = &mut self.checkpoint {
            checkpoint(Stage::BeforeRename).map_err(|error| self.failed("before rename", error))?;
        }
        // Rename is the commit boundary: later errors cannot promise the old value survived.
        temporary
            .persist(&self.path)
            .map_err(|error| self.failed("replace", error.error))?;
        #[cfg(test)]
        if let Some(checkpoint) = &mut self.checkpoint {
            checkpoint(Stage::AfterRename).map_err(|error| self.uncertain_commit(error))?;
        }
        directory
            .sync_all()
            .map_err(|error| self.uncertain_commit(error))
    }
    #[cfg(not(unix))]
    pub fn save(&mut self, _: &[u8]) -> Result<(), ControlError> {
        Err(ControlError::Unavailable(
            "durable settings replacement is implemented only on Unix",
        ))
    }
}
