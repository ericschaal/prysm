//! TV-local preferences (`tv.json`): one writer, the supervisor.
use crate::state::Flavor;
use prysm_webos_link::{LightingTarget, PrysmAddress};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Largest preferences file read back.
pub const MAX_BYTES: usize = 65_536;

/// The installed package's preferences: the `setPreferences` request
/// (complete, every field optional, an unset one absent) and the published
/// `preferences`. Fields marked "Slim only" must stay unset in the full
/// package. The file is [`PreferencesFile`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(remote = "Self", deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct Preferences {
    /// Start when the service starts: the full package starts screen sync,
    /// the slim package streams to its receiver.
    #[serde(default)]
    pub autostart: bool,
    /// Slim only: the headless Prysm's address, such as
    /// `http://192.168.1.20:8787` or `https://prysm.example.com` (through a
    /// reverse proxy). The TV streams its picture there, and the TV app
    /// links to its portal. Only the syntax is checked when saved; the TV
    /// resolves a host name at every connection attempt, and one that does
    /// not resolve is reported as the receiver wait.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "schema",
        schema(value_type = Option<String>, nullable = false, max_length = 256, example = "http://192.168.1.20:8787")
    )]
    pub prysm_address: Option<PrysmAddress>,
}
prysm_ports::map_only!(Preferences, "preferences as an object");

impl Preferences {
    /// # Errors
    /// A slim-only field in the full package, as `field: reason`.
    pub fn validate(&self, flavor: Flavor) -> Result<(), String> {
        // `prysm_address` was checked when it was parsed.
        if flavor == Flavor::Full && self.prysm_address.is_some() {
            return Err("prysm_address: only the slim package forwards frames".into());
        }
        Ok(())
    }
}

/// `tv.json` as read and written: both packages' preferences, and every
/// field this version does not know, kept as read. Both packages install as
/// the same app and share the file, so the full package keeps the slim
/// package's address (as read, unchecked) for a switch back.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreferencesFile {
    #[serde(default)]
    autostart: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prysm_address: Option<Value>,
    #[serde(flatten)]
    other: Map<String, Value>,
}

impl PreferencesFile {
    /// The fields `flavor` owns.
    ///
    /// # Errors
    /// Slim: the saved address is not a Prysm address.
    pub fn preferences(&self, flavor: Flavor) -> Result<Preferences, serde_json::Error> {
        let prysm_address = match flavor {
            Flavor::Full => None,
            Flavor::Slim => self
                .prysm_address
                .clone()
                .map(serde_json::from_value)
                .transpose()?,
        };
        Ok(Preferences {
            autostart: self.autostart,
            prysm_address,
        })
    }

    /// Replace the fields `flavor` owns with `preferences` (validated for
    /// it); every other field stays as read.
    pub fn replace(&mut self, preferences: &Preferences, flavor: Flavor) {
        self.autostart = preferences.autostart;
        if flavor == Flavor::Slim {
            self.prysm_address = preferences
                .prysm_address
                .as_ref()
                .map(|address| Value::String(address.to_string()));
        }
    }
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
    fn load(&mut self) -> Result<Option<PreferencesFile>, StoreError>;
    /// # Errors
    /// The file could not be replaced.
    fn save(&mut self, file: &PreferencesFile) -> Result<(), StoreError>;
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

/// In memory, for tests: the preferences file and the remembered intent.
#[cfg(any(test, feature = "testkit"))]
#[derive(Debug, Clone, Default)]
pub struct MemoryStore(pub Option<PreferencesFile>, pub Option<LightingTarget>);

#[cfg(any(test, feature = "testkit"))]
impl PreferencesStore for MemoryStore {
    fn load(&mut self) -> Result<Option<PreferencesFile>, StoreError> {
        Ok(self.0.clone())
    }
    fn save(&mut self, file: &PreferencesFile) -> Result<(), StoreError> {
        self.0 = Some(file.clone());
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

/// Write `bytes` to `path` atomically, private (0600).
fn replace(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    write_atomic(path, bytes, 0o600, true).map_err(|source| StoreError::Save {
        path: path.to_owned(),
        source,
    })
}

/// Write `bytes` with `mode` to a temporary file next to `path`, sync it,
/// rename it to `path` and sync the directory: a reader sees the old file or
/// the new one, never a partial write. Without `replace`, an existing `path`
/// is an error and stays as it is.
///
/// # Errors
/// Any step failed. An error before the rename removes the temporary file;
/// a crash can leave it behind (a dot-file next to `path`).
pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32, replace: bool) -> io::Result<()> {
    let directory = path.parent().unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(mode))?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    if replace {
        file.persist(path).map_err(|error| error.error)?;
    } else {
        file.persist_noclobber(path).map_err(|error| error.error)?;
    }
    fs::File::open(directory)?.sync_all()
}

impl PreferencesStore for FileStore {
    fn load(&mut self) -> Result<Option<PreferencesFile>, StoreError> {
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

    fn save(&mut self, file: &PreferencesFile) -> Result<(), StoreError> {
        let mut bytes = serde_json::to_vec_pretty(file).map_err(|error| StoreError::Save {
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
            prysm_address: Some("http://192.168.1.20:8787".parse().unwrap()),
        };
        slim.validate(Flavor::Slim).unwrap();
        assert!(
            slim.validate(Flavor::Full)
                .unwrap_err()
                .starts_with("prysm_address")
        );
        Preferences::default().validate(Flavor::Full).unwrap();
        Preferences::default().validate(Flavor::Slim).unwrap();
    }

    #[test]
    fn the_intent_resumes_within_one_boot_only() {
        let dir = tempfile::tempdir().unwrap();
        let boot = dir.path().join("boot_id");
        fs::write(&boot, "boot-1\n").unwrap();
        let mut store = FileStore::new(dir.path().join("tv.json")).with_boot_id(&boot);
        assert_eq!(store.resume(), None);
        store.remember(LightingTarget::Effects).unwrap();
        assert_eq!(store.resume(), Some(LightingTarget::Effects));
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
        store.remember(LightingTarget::ScreenSync).unwrap();
        assert_eq!(store.resume(), None);
    }

    #[test]
    fn the_address_is_checked_when_read() {
        let parsed: Preferences =
            serde_json::from_str(r#"{"prysm_address":"https://prysm.example.com/"}"#).unwrap();
        assert_eq!(
            serde_json::to_value(&parsed).unwrap()["prysm_address"],
            "https://prysm.example.com"
        );
        for bad in ["prysm.local:7380", "http://0.0.0.0:8787", "http://x/path"] {
            let error =
                serde_json::from_value::<Preferences>(serde_json::json!({ "prysm_address": bad }))
                    .unwrap_err()
                    .to_string();
            assert!(
                error.starts_with(&format!("Prysm address `{bad}`: ")),
                "{error}"
            );
        }
    }

    /// Every field has a default, so serde's derive alone would read `[]`
    /// (and `[true]`) as preferences.
    #[test]
    fn only_an_object_is_preferences() {
        for refused in ["[]", "[true]", "null", "true"] {
            let error = serde_json::from_str::<Preferences>(refused)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("preferences as an object"),
                "{refused}: {error}"
            );
        }
    }

    #[test]
    fn unknown_fields_are_rejected_in_a_request() {
        assert!(serde_json::from_str::<Preferences>(r#"{"mode":"slim"}"#).is_err());
        // Picture settings are the receiver's (slim) or the portal's (full).
        assert!(serde_json::from_str::<Preferences>(r#"{"capture":{"fps":30}}"#).is_err());
        let parsed: Preferences = serde_json::from_str(r#"{"autostart":true}"#).unwrap();
        assert!(parsed.autostart);
    }

    /// The file is shared by both packages and by other versions: each reads
    /// its own fields and writes every other field back as it read it.
    #[test]
    fn the_file_keeps_what_the_package_does_not_own() {
        let read = |json: Value| serde_json::from_value::<PreferencesFile>(json).unwrap();
        let address: PrysmAddress = "http://192.168.1.20:8787".parse().unwrap();
        let mut file = read(serde_json::json!({
            "autostart": true,
            "prysm_address": "http://192.168.1.20:8787",
            "newer": { "x": 1 },
        }));
        // Full reads `autostart` and keeps the address it ignores.
        assert_eq!(
            file.preferences(Flavor::Full).unwrap(),
            Preferences {
                autostart: true,
                prysm_address: None
            }
        );
        file.replace(&Preferences::default(), Flavor::Full);
        assert_eq!(
            serde_json::to_value(&file).unwrap(),
            serde_json::json!({
                "autostart": false,
                "prysm_address": "http://192.168.1.20:8787",
                "newer": { "x": 1 },
            })
        );
        // Slim reads and replaces both.
        assert_eq!(
            file.preferences(Flavor::Slim).unwrap().prysm_address,
            Some(address)
        );
        file.replace(&Preferences::default(), Flavor::Slim);
        assert_eq!(
            serde_json::to_value(&file).unwrap(),
            serde_json::json!({ "autostart": false, "newer": { "x": 1 } })
        );
        // An address full ignores is never checked there; slim refuses it.
        let file = read(serde_json::json!({ "prysm_address": "http://x/path" }));
        file.preferences(Flavor::Full).unwrap();
        assert!(file.preferences(Flavor::Slim).is_err());
        // A known field of the wrong type is unreadable in both.
        assert!(serde_json::from_str::<PreferencesFile>(r#"{"autostart":1}"#).is_err());
    }

    #[test]
    fn file_store_round_trips_privately_and_refuses_odd_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tv.json");
        let mut store = FileStore::new(&path);
        assert_eq!(store.load().unwrap(), None);
        let mut file = PreferencesFile::default();
        file.replace(
            &Preferences {
                autostart: true,
                ..Preferences::default()
            },
            Flavor::Full,
        );
        store.save(&file).unwrap();
        assert_eq!(store.load().unwrap(), Some(file));
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
