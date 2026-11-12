//! TV-local preferences (`tv.json`): one writer, the supervisor.
use crate::state::Flavor;
use prysm_webos_link::SlimCapture;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Read, Write};
use std::net::SocketAddr;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Largest preferences file read back.
pub const MAX_BYTES: usize = 65_536;

/// `tv.json` and the `setPreferences` request (complete, every field
/// optional). Fields marked "Slim only" must stay unset (default) in the
/// full package.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Preferences {
    /// Start enabled (full: processed lighting) when the service starts.
    pub autostart: bool,
    /// Slim only: the headless Prysm's portal origin, such as
    /// `http://192.168.1.20:8787`. Only the TV app uses it.
    #[cfg_attr(
        feature = "schema",
        schema(max_length = 256, example = "http://192.168.1.20:8787")
    )]
    pub instance: Option<String>,
    /// Slim only: where frames go, such as `192.168.1.20:7380`.
    #[cfg_attr(
        feature = "schema",
        schema(value_type = Option<String>, example = "192.168.1.20:7380")
    )]
    pub receiver: Option<SocketAddr>,
    /// Slim only: capture size, rate and menu blending.
    #[cfg_attr(feature = "schema", schema(value_type = crate::schema::SlimCaptureSchema))]
    pub capture: SlimCapture,
}

impl Preferences {
    /// # Errors
    /// The first invalid field, as `field: reason`.
    pub fn validate(&self, flavor: Flavor) -> Result<(), String> {
        if flavor == Flavor::Full {
            if self.instance.is_some() || self.receiver.is_some() {
                return Err("instance, receiver: only the slim package forwards frames".into());
            }
            if self.capture != SlimCapture::default() {
                return Err("capture: set in the portal for the full package".into());
            }
            return Ok(());
        }
        if let Some(instance) = &self.instance {
            origin(instance).map_err(|reason| format!("instance: {reason}"))?;
        }
        if let Some(receiver) = self.receiver
            && (receiver.port() == 0 || receiver.ip().is_unspecified())
        {
            return Err("receiver: needs a host and a fixed port".into());
        }
        self.capture.validate()
    }
}

/// `http(s)://host[:port]` with nothing after it.
fn origin(text: &str) -> Result<(), String> {
    let rest = text
        .strip_prefix("http://")
        .or_else(|| text.strip_prefix("https://"))
        .ok_or("must start with http:// or https://")?;
    if text.len() > 256 {
        return Err("too long".into());
    }
    if rest.is_empty()
        || rest
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "/?#@\\".contains(c))
    {
        return Err("must be an origin such as http://192.168.1.20:8787".into());
    }
    Ok(())
}

/// Loads and saves preferences.
pub trait PreferencesStore {
    /// `Ok(None)`: nothing saved yet.
    ///
    /// # Errors
    /// Unreadable, oversized or invalid content.
    fn load(&mut self) -> Result<Option<Preferences>, String>;
    /// # Errors
    /// The file could not be replaced.
    fn save(&mut self, preferences: &Preferences) -> Result<(), String>;
}

/// In memory, for tests and probes.
#[derive(Debug, Clone, Default)]
pub struct MemoryStore(pub Option<Preferences>);

impl PreferencesStore for MemoryStore {
    fn load(&mut self) -> Result<Option<Preferences>, String> {
        Ok(self.0.clone())
    }
    fn save(&mut self, preferences: &Preferences) -> Result<(), String> {
        self.0 = Some(preferences.clone());
        Ok(())
    }
}

/// A JSON file in a private directory, replaced atomically (0600).
#[derive(Debug, Clone)]
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn directory(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new("."))
    }
}

impl PreferencesStore for FileStore {
    fn load(&mut self) -> Result<Option<Preferences>, String> {
        let failed = |error: io::Error| format!("cannot read {}: {error}", self.path.display());
        let Some(bytes) = read_private(&self.path, MAX_BYTES).map_err(failed)? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("invalid {}: {error}", self.path.display()))
    }

    fn save(&mut self, preferences: &Preferences) -> Result<(), String> {
        let failed = |error: io::Error| format!("cannot save {}: {error}", self.path.display());
        let mut bytes = serde_json::to_vec_pretty(preferences).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        let mut file = tempfile::NamedTempFile::new_in(self.directory()).map_err(failed)?;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(failed)?;
        file.write_all(&bytes).map_err(failed)?;
        file.as_file().sync_all().map_err(failed)?;
        file.persist(&self.path)
            .map_err(|error| failed(error.error))?;
        fs::File::open(self.directory())
            .and_then(|directory| directory.sync_all())
            .map_err(failed)
    }
}

/// Read a regular file without following links or blocking on a FIFO.
/// `Ok(None)`: missing.
///
/// # Errors
/// Not a regular file, larger than `limit`, or unreadable.
pub fn read_private(path: &Path, limit: usize) -> io::Result<Option<Vec<u8>>> {
    use nix::fcntl::OFlag;
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags((OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK).bits())
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::other(format!("larger than {limit} bytes")));
    }
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_rejects_slim_fields_and_slim_validates_them() {
        let slim = Preferences {
            autostart: true,
            instance: Some("http://192.168.1.20:8787".into()),
            receiver: Some("192.168.1.20:7380".parse().unwrap()),
            capture: SlimCapture::default(),
        };
        slim.validate(Flavor::Slim).unwrap();
        assert!(slim.validate(Flavor::Full).is_err());
        Preferences::default().validate(Flavor::Full).unwrap();
        Preferences::default().validate(Flavor::Slim).unwrap();

        for (bad, field) in [
            (
                Preferences {
                    instance: Some("ftp://x".into()),
                    ..slim.clone()
                },
                "instance",
            ),
            (
                Preferences {
                    instance: Some("http://x/path".into()),
                    ..slim.clone()
                },
                "instance",
            ),
            (
                Preferences {
                    instance: Some("http://".into()),
                    ..slim.clone()
                },
                "instance",
            ),
            (
                Preferences {
                    receiver: Some("0.0.0.0:7380".parse().unwrap()),
                    ..slim.clone()
                },
                "receiver",
            ),
            (
                Preferences {
                    receiver: Some("10.0.0.1:0".parse().unwrap()),
                    ..slim.clone()
                },
                "receiver",
            ),
            (
                Preferences {
                    capture: SlimCapture {
                        fps: 0,
                        ..SlimCapture::default()
                    },
                    ..slim.clone()
                },
                "capture.fps",
            ),
        ] {
            assert!(
                bad.validate(Flavor::Slim).unwrap_err().starts_with(field),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn unknown_fields_are_rejected() {
        assert!(serde_json::from_str::<Preferences>(r#"{"mode":"slim"}"#).is_err());
        let parsed: Preferences = serde_json::from_str(r#"{"autostart":true}"#).unwrap();
        assert!(parsed.autostart);
        assert_eq!(parsed.capture, SlimCapture::default());
    }

    #[test]
    fn file_store_round_trips_privately_and_refuses_odd_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tv.json");
        let mut store = FileStore::new(&path);
        assert_eq!(store.load().unwrap(), None);
        let preferences = Preferences {
            autostart: true,
            ..Preferences::default()
        };
        store.save(&preferences).unwrap();
        assert_eq!(store.load().unwrap(), Some(preferences));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        fs::write(&path, vec![b' '; MAX_BYTES + 1]).unwrap();
        assert!(store.load().unwrap_err().contains("larger"));
        fs::write(&path, "{\"autostart\":1}").unwrap();
        assert!(store.load().unwrap_err().contains("invalid"));
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(dir.path().join("elsewhere"), &path).unwrap();
        assert!(store.load().is_err(), "links are never followed");
        fs::remove_file(&path).unwrap();
        nix::unistd::mkfifo(&path, nix::sys::stat::Mode::S_IRUSR).unwrap();
        assert!(store.load().unwrap_err().contains("regular"));
    }
}
