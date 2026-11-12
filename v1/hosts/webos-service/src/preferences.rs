//! TV-local preferences (`tv.json`): one writer, the supervisor.
use crate::state::Flavor;
use prysm_webos_link::{LightingTarget, SlimCapture};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Read, Write};
use std::net::SocketAddr;
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
    /// Slim only: where frames go, such as `192.168.1.20:7380`.
    #[cfg_attr(
        feature = "schema",
        schema(value_type = Option<String>, example = "192.168.1.20:7380")
    )]
    pub receiver: Option<SocketAddr>,
    /// Slim only: capture size, rate, menu blending and encoding. Always set
    /// in the slim package (a request without it gets the defaults); `null`
    /// in the full package, whose capture is the portal's `settings.capture`.
    #[cfg_attr(
        feature = "schema",
        schema(value_type = Option<crate::schema::SlimCaptureSchema>)
    )]
    pub capture: Option<SlimCapture>,
}

impl Preferences {
    /// The preferences a package starts with: slim with the default capture.
    #[must_use]
    pub fn defaults(flavor: Flavor) -> Self {
        Self::default().for_flavor(flavor)
    }

    /// Fill what the flavor always has: a slim capture left out of a request
    /// or file becomes the default one. Full is unchanged (validation
    /// rejects a capture there).
    #[must_use]
    pub fn for_flavor(mut self, flavor: Flavor) -> Self {
        if flavor == Flavor::Slim {
            self.capture.get_or_insert_with(SlimCapture::default);
        }
        self
    }

    /// # Errors
    /// The first invalid field, as `field: reason`.
    pub fn validate(&self, flavor: Flavor) -> Result<(), String> {
        if flavor == Flavor::Full {
            if self.instance.is_some() || self.receiver.is_some() {
                return Err("instance, receiver: only the slim package forwards frames".into());
            }
            if self.capture.is_some() {
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
        self.capture.as_ref().map_or(Ok(()), SlimCapture::validate)
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
    /// The intent remembered earlier in this boot (`Machine::intent`), so a
    /// restart of the service (an update, a crash) resumes it; `None` after
    /// a reboot, where `autostart` decides.
    fn resume(&mut self) -> Option<LightingTarget>;
    /// Remember the intent for [`PreferencesStore::resume`].
    ///
    /// # Errors
    /// The file could not be replaced.
    fn remember(&mut self, intent: LightingTarget) -> Result<(), String>;
}

/// In memory, for tests and probes: preferences and the remembered intent.
#[derive(Debug, Clone, Default)]
pub struct MemoryStore(pub Option<Preferences>, pub Option<LightingTarget>);

impl PreferencesStore for MemoryStore {
    fn load(&mut self) -> Result<Option<Preferences>, String> {
        Ok(self.0.clone())
    }
    fn save(&mut self, preferences: &Preferences) -> Result<(), String> {
        self.0 = Some(preferences.clone());
        Ok(())
    }
    fn resume(&mut self) -> Option<LightingTarget> {
        self.1
    }
    fn remember(&mut self, intent: LightingTarget) -> Result<(), String> {
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
fn replace(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let failed = |error: io::Error| format!("cannot save {}: {error}", path.display());
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
        let mut bytes = serde_json::to_vec_pretty(preferences).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        replace(&self.path, &bytes)
    }

    fn resume(&mut self) -> Option<LightingTarget> {
        let boot = self.boot()?;
        let bytes = read_private(&self.resume_path(), 1024).ok()??;
        let resume: Resume = serde_json::from_slice(&bytes).ok()?;
        (resume.boot == boot).then_some(resume.intent)
    }

    fn remember(&mut self, intent: LightingTarget) -> Result<(), String> {
        let Some(boot) = self.boot() else {
            return Ok(());
        };
        let bytes = serde_json::to_vec(&Resume { boot, intent }).map_err(|e| e.to_string())?;
        replace(&self.resume_path(), &bytes)
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
            capture: Some(SlimCapture::default()),
        };
        slim.validate(Flavor::Slim).unwrap();
        assert!(slim.validate(Flavor::Full).is_err());
        Preferences::default().validate(Flavor::Full).unwrap();
        Preferences::default().validate(Flavor::Slim).unwrap();
        // Any capture in the full package, even the slim default, is refused.
        let full_capture = Preferences {
            capture: Some(SlimCapture::default()),
            ..Preferences::default()
        };
        assert!(
            full_capture
                .validate(Flavor::Full)
                .unwrap_err()
                .starts_with("capture")
        );

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
                    capture: Some(SlimCapture {
                        fps: 0,
                        ..SlimCapture::default()
                    }),
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
    fn capture_is_null_in_full_and_always_set_in_slim() {
        let full = Preferences::defaults(Flavor::Full);
        assert_eq!(full.capture, None);
        assert_eq!(
            serde_json::to_value(&full).unwrap()["capture"],
            serde_json::Value::Null
        );

        let slim = Preferences::defaults(Flavor::Slim);
        assert_eq!(slim.capture, Some(SlimCapture::default()));
        let value = serde_json::to_value(&slim).unwrap();
        assert_eq!(value["capture"]["width"], 320, "the default is written out");
        assert_eq!(serde_json::from_value::<Preferences>(value).unwrap(), slim);

        // A slim request or file without a capture gets the default one.
        let parsed: Preferences = serde_json::from_str(r#"{"autostart":true}"#).unwrap();
        assert_eq!(parsed.capture, None);
        assert_eq!(
            parsed.clone().for_flavor(Flavor::Slim).capture,
            Some(SlimCapture::default())
        );
        assert_eq!(parsed.for_flavor(Flavor::Full).capture, None);
        let chosen = Preferences {
            capture: Some(SlimCapture {
                width: 640,
                height: 360,
                ..SlimCapture::default()
            }),
            ..Preferences::default()
        };
        assert_eq!(chosen.clone().for_flavor(Flavor::Slim), chosen);
    }

    #[test]
    fn unknown_fields_are_rejected() {
        assert!(serde_json::from_str::<Preferences>(r#"{"mode":"slim"}"#).is_err());
        let parsed: Preferences = serde_json::from_str(r#"{"autostart":true}"#).unwrap();
        assert!(parsed.autostart);
        assert_eq!(parsed.capture, None);
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
