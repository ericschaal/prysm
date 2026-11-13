//! TV-local preferences (`tv.json`): one writer, the supervisor.
use crate::state::Flavor;
use prysm_webos_link::{LightingTarget, ReceiverAddress};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Largest preferences file read back.
pub const MAX_BYTES: usize = 65_536;

/// `tv.json` and the `setPreferences` request (complete, every field
/// optional). Fields marked "Slim only" must stay unset (`null` or absent)
/// in the full package.
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
    /// Slim only: where frames go, `host:port`: an IP address (IPv6 in
    /// brackets) or a host name, such as `192.168.1.20:7380` or
    /// `prysm.local:7380`, with a fixed port. Only the syntax is checked when
    /// saved; the TV resolves a name at every connection attempt, and one
    /// that does not resolve is reported as the receiver wait.
    #[cfg_attr(
        feature = "schema",
        schema(value_type = Option<String>, max_length = 259, example = "prysm.local:7380")
    )]
    pub receiver: Option<ReceiverAddress>,
}

impl Preferences {
    /// # Errors
    /// The first invalid field, as `field: reason`.
    pub fn validate(&self, flavor: Flavor) -> Result<(), String> {
        if flavor == Flavor::Full {
            if self.instance.is_some() || self.receiver.is_some() {
                return Err("instance, receiver: only the slim package forwards frames".into());
            }
            return Ok(());
        }
        // `receiver` was checked when it was parsed.
        if let Some(instance) = &self.instance {
            origin(instance).map_err(|reason| format!("instance: {reason}"))?;
        }
        Ok(())
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

/// Why preferences or the intent could not be read or saved.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("cannot read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid {}", path.display())]
    Invalid {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("cannot save {}", path.display())]
    Save {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Loads and saves preferences.
pub trait PreferencesStore {
    /// `Ok(None)`: nothing saved yet.
    ///
    /// # Errors
    /// Unreadable, oversized or invalid content.
    fn load(&mut self) -> Result<Option<Preferences>, StoreError>;
    /// # Errors
    /// The file could not be replaced.
    fn save(&mut self, preferences: &Preferences) -> Result<(), StoreError>;
    /// The intent remembered earlier in this boot (`Machine::intent`), so a
    /// restart of the service (an update, a crash) resumes it; `None` after
    /// a reboot, where `autostart` decides.
    fn resume(&mut self) -> Option<LightingTarget>;
    /// Remember the intent for [`PreferencesStore::resume`].
    ///
    /// # Errors
    /// The file could not be replaced.
    fn remember(&mut self, intent: LightingTarget) -> Result<(), StoreError>;
}

/// In memory, for tests and probes: preferences and the remembered intent.
#[derive(Debug, Clone, Default)]
pub struct MemoryStore(pub Option<Preferences>, pub Option<LightingTarget>);

impl PreferencesStore for MemoryStore {
    fn load(&mut self) -> Result<Option<Preferences>, StoreError> {
        Ok(self.0.clone())
    }
    fn save(&mut self, preferences: &Preferences) -> Result<(), StoreError> {
        self.0 = Some(preferences.clone());
        Ok(())
    }
    fn resume(&mut self) -> Option<LightingTarget> {
        self.1
    }
    fn remember(&mut self, intent: LightingTarget) -> Result<(), StoreError> {
        self.1 = Some(intent);
        Ok(())
    }
}

/// The kernel's id of the current boot.
pub const BOOT_ID: &str = "/proc/sys/kernel/random/boot_id";

/// `resume.json`: the intent and the boot it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Resume {
    boot: String,
    intent: LightingTarget,
}

/// A JSON file in a private directory, replaced atomically (0600), and
/// `resume.json` next to it for the intent of the current boot.
#[derive(Debug, Clone)]
pub struct FileStore {
    path: PathBuf,
    boot_id: PathBuf,
}

impl FileStore {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            boot_id: BOOT_ID.into(),
        }
    }

    /// Read the boot id from `path` instead of [`BOOT_ID`] (tests).
    #[must_use]
    pub fn with_boot_id(mut self, path: impl Into<PathBuf>) -> Self {
        self.boot_id = path.into();
        self
    }

    fn directory(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new("."))
    }

    fn resume_path(&self) -> PathBuf {
        self.directory().join("resume.json")
    }

    /// `None` where the kernel has no boot id (not Linux): nothing resumes.
    fn boot(&self) -> Option<String> {
        let bytes = fs::read(&self.boot_id).ok()?;
        let boot = String::from_utf8(bytes).ok()?.trim().to_owned();
        (!boot.is_empty() && boot.len() <= 64).then_some(boot)
    }
}

/// Write `bytes` to `path` atomically, private (0600), and sync the directory.
fn replace(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let failed = |source: io::Error| StoreError::Save {
        path: path.to_owned(),
        source,
    };
    let directory = path.parent().unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(directory).map_err(failed)?;
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(failed)?;
    file.write_all(bytes).map_err(failed)?;
    file.as_file().sync_all().map_err(failed)?;
    file.persist(path).map_err(|error| failed(error.error))?;
    fs::File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(failed)
}

impl PreferencesStore for FileStore {
    fn load(&mut self) -> Result<Option<Preferences>, StoreError> {
        let path = &self.path;
        let Some(bytes) = read_private(path, MAX_BYTES).map_err(|source| StoreError::Read {
            path: path.clone(),
            source,
        })?
        else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|source| StoreError::Invalid {
                path: path.clone(),
                source,
            })
    }

    fn save(&mut self, preferences: &Preferences) -> Result<(), StoreError> {
        let mut bytes =
            serde_json::to_vec_pretty(preferences).map_err(|error| StoreError::Save {
                path: self.path.clone(),
                source: error.into(),
            })?;
        bytes.push(b'\n');
        replace(&self.path, &bytes)
    }

    fn resume(&mut self) -> Option<LightingTarget> {
        let boot = self.boot()?;
        let bytes = read_private(&self.resume_path(), 1024).ok()??;
        let resume: Resume = serde_json::from_slice(&bytes).ok()?;
        (resume.boot == boot).then_some(resume.intent)
    }

    fn remember(&mut self, intent: LightingTarget) -> Result<(), StoreError> {
        let Some(boot) = self.boot() else {
            return Ok(());
        };
        let path = self.resume_path();
        let bytes =
            serde_json::to_vec(&Resume { boot, intent }).map_err(|error| StoreError::Save {
                path: path.clone(),
                source: error.into(),
            })?;
        replace(&path, &bytes)
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
        };
        slim.validate(Flavor::Slim).unwrap();
        Preferences {
            receiver: Some("prysm.local:7380".parse().unwrap()),
            ..slim.clone()
        }
        .validate(Flavor::Slim)
        .unwrap();
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
        ] {
            assert!(
                bad.validate(Flavor::Slim).unwrap_err().starts_with(field),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn the_intent_resumes_within_one_boot_only() {
        let dir = tempfile::tempdir().unwrap();
        let boot = dir.path().join("boot_id");
        fs::write(&boot, "boot-1\n").unwrap();
        let mut store = FileStore::new(dir.path().join("tv.json")).with_boot_id(&boot);
        assert_eq!(store.resume(), None);
        store.remember(LightingTarget::DeviceNative).unwrap();
        assert_eq!(store.resume(), Some(LightingTarget::DeviceNative));
        assert_eq!(
            fs::metadata(dir.path().join("resume.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        // A reboot: autostart decides again.
        fs::write(&boot, "boot-2\n").unwrap();
        assert_eq!(store.resume(), None);
        // Without a boot id nothing is kept.
        let mut store =
            FileStore::new(dir.path().join("tv.json")).with_boot_id(dir.path().join("missing"));
        store.remember(LightingTarget::Processed).unwrap();
        assert_eq!(store.resume(), None);
    }

    #[test]
    fn receivers_are_checked_when_read() {
        let parsed: Preferences =
            serde_json::from_str(r#"{"receiver":"prysm.local:7380"}"#).unwrap();
        assert_eq!(
            serde_json::to_value(&parsed).unwrap()["receiver"],
            "prysm.local:7380"
        );
        for bad in ["0.0.0.0:7380", "10.0.0.1:0", "prysm.local", "http://x:1"] {
            let error =
                serde_json::from_value::<Preferences>(serde_json::json!({ "receiver": bad }))
                    .unwrap_err()
                    .to_string();
            assert!(error.starts_with(&format!("receiver `{bad}`: ")), "{error}");
        }
    }

    #[test]
    fn unknown_fields_are_rejected() {
        assert!(serde_json::from_str::<Preferences>(r#"{"mode":"slim"}"#).is_err());
        // Picture settings are the receiver's (slim) or the portal's (full).
        assert!(serde_json::from_str::<Preferences>(r#"{"capture":{"fps":30}}"#).is_err());
        let parsed: Preferences = serde_json::from_str(r#"{"autostart":true}"#).unwrap();
        assert!(parsed.autostart);
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
        let error = |store: &mut FileStore| prysm_ports::chain(&store.load().unwrap_err());
        assert!(error(&mut store).contains("larger"));
        fs::write(&path, "{\"autostart\":1}").unwrap();
        assert!(error(&mut store).contains("invalid"));
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(dir.path().join("elsewhere"), &path).unwrap();
        assert!(store.load().is_err(), "links are never followed");
        fs::remove_file(&path).unwrap();
        nix::unistd::mkfifo(&path, nix::sys::stat::Mode::S_IRUSR).unwrap();
        assert!(error(&mut store).contains("regular"));
    }
}
