//! Replacing a file atomically: a reader sees the old file or the new one,
//! never a partial write. [`write`] does it in one call; [`Staged`] is its
//! steps, for a caller that acts between the write and the rename.
use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// The directory `path` is in: its parent, or `.` for a bare file name.
#[must_use]
pub fn directory(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

/// Write `bytes` through a [`Staged`] file, then sync the directory. `None`
/// keeps the temporary file's private permissions. With `replace`, `path` is
/// replaced (a symbolic link there too, not the file it names). Without it,
/// an existing `path` is an error and stays as it is.
///
/// # Errors
/// Any step failed. A failure before the rename leaves `path` as it was and
/// removes the temporary file; a failed directory sync comes after the
/// rename, so `path` already holds the new file.
pub fn write(
    path: &Path,
    bytes: &[u8],
    permissions: impl Into<Option<fs::Permissions>>,
    replace: bool,
) -> io::Result<()> {
    let mut staged = Staged::new(path)?;
    if let Some(permissions) = permissions.into() {
        staged.set_permissions(permissions)?;
    }
    staged.write(bytes)?;
    if replace {
        staged.replace(path)?;
    } else {
        staged.create(path)?;
    }
    sync_directory(directory(path))
}

/// A new file beside the one it is to become, under a random name in the
/// same directory (so the rename stays within one filesystem), owner-only
/// until [`set_permissions`](Self::set_permissions). Removed when dropped
/// before [`replace`](Self::replace) or [`create`](Self::create) succeeds;
/// a crash can leave it behind (a dot-file next to the path).
#[derive(Debug)]
pub struct Staged(tempfile::NamedTempFile);

impl Staged {
    /// # Errors
    /// The file cannot be created in `path`'s directory.
    pub fn new(path: &Path) -> io::Result<Self> {
        tempfile::NamedTempFile::new_in(directory(path)).map(Self)
    }

    /// # Errors
    /// The permissions cannot be set.
    pub fn set_permissions(&self, permissions: fs::Permissions) -> io::Result<()> {
        self.0.as_file().set_permissions(permissions)
    }

    /// Write all of `bytes` and sync them to the disk.
    ///
    /// # Errors
    /// The write or the sync failed.
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.0.write_all(bytes)?;
        self.0.as_file().sync_all()
    }

    /// Rename the file to `path`, replacing whatever is there; a symbolic
    /// link is replaced, not followed.
    ///
    /// # Errors
    /// The rename failed; the file is removed.
    pub fn replace(self, path: &Path) -> io::Result<()> {
        self.0.persist(path).map(drop).map_err(|error| error.error)
    }

    /// Rename the file to `path`, which must not exist.
    ///
    /// # Errors
    /// Something is at `path`, or the rename failed; the file is removed.
    pub fn create(self, path: &Path) -> io::Result<()> {
        self.0
            .persist_noclobber(path)
            .map(drop)
            .map_err(|error| error.error)
    }
}

/// Sync `directory`, so that a rename in it survives a crash.
///
/// # Errors
/// It cannot be opened or synced.
#[cfg(unix)]
pub fn sync_directory(directory: &Path) -> io::Result<()> {
    fs::File::open(directory)?.sync_all()
}

/// Other platforms cannot open a directory for syncing; the rename is still atomic.
///
/// # Errors
/// None.
#[cfg(not(unix))]
pub fn sync_directory(_: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn entries(directory: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn write_creates_and_replaces_with_the_permissions_given() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file");
        write(&path, b"one", fs::Permissions::from_mode(0o640), false).unwrap();
        assert_eq!(
            (fs::read(&path).unwrap(), mode(&path)),
            (b"one".to_vec(), 0o640)
        );
        write(&path, b"two", fs::Permissions::from_mode(0o600), true).unwrap();
        assert_eq!(
            (fs::read(&path).unwrap(), mode(&path)),
            (b"two".to_vec(), 0o600)
        );
        assert_eq!(entries(directory.path()), ["file"]);
    }

    #[test]
    fn without_replace_an_existing_file_stays() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file");
        fs::write(&path, "kept").unwrap();
        let error = write(&path, b"new", fs::Permissions::from_mode(0o600), false).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(&path).unwrap(), "kept");
        assert_eq!(entries(directory.path()), ["file"]);
    }

    #[test]
    fn a_symbolic_link_is_replaced_not_followed() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        fs::write(&target, "target").unwrap();
        let path = directory.path().join("link");
        symlink(&target, &path).unwrap();
        write(&path, b"new", fs::Permissions::from_mode(0o600), true).unwrap();
        assert!(!fs::symlink_metadata(&path).unwrap().is_symlink());
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(fs::read_to_string(&target).unwrap(), "target");
    }

    #[test]
    fn a_staged_file_is_private_and_removed_when_dropped() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file");
        let mut staged = Staged::new(&path).unwrap();
        staged.write(b"x").unwrap();
        assert_eq!(mode(staged.0.path()), 0o600);
        drop(staged);
        assert!(entries(directory.path()).is_empty());
    }

    #[test]
    fn a_bare_file_name_is_in_the_working_directory() {
        assert_eq!(directory(Path::new("settings.json")), Path::new("."));
        assert_eq!(directory(Path::new("a/settings.json")), Path::new("a"));
    }
}
