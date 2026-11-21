//! Commands typed in the terminal, executed against the engine.
use crate::input::Notice;
use prysm_ports::{Error, LightingTarget, Result, Settings, SettingsStore, chain};
use prysm_runtime::EngineHandle;
use prysm_settings_file::SettingsFile;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    SetTarget(LightingTarget),
    /// Replace all settings with a settings file, against the revision shown.
    Load(PathBuf),
}

/// Run one command and describe its outcome for the diagnostics panel.
pub(crate) async fn execute(engine: EngineHandle, command: Command) -> Notice {
    match command {
        Command::SetTarget(target) => match engine.set_target(target).await {
            Ok(()) => Notice::info(format!("Target {}", target.as_str())),
            Err(error) => Notice::error(format!("Target {}: {}", target.as_str(), chain(&error))),
        },
        Command::Load(path) => {
            let shown = path.display().to_string();
            match load(&engine, path).await {
                Ok(revision) => Notice::info(format!("Settings revision {revision} from {shown}")),
                Err(error) => Notice::error(format!("Load {shown}: {}", chain(&error))),
            }
        }
    }
}

async fn load(engine: &EngineHandle, path: PathBuf) -> Result<prysm_ports::Revision> {
    let expected = engine.snapshot().borrow().revision;
    let settings = tokio::task::spawn_blocking(move || read_settings(&path))
        .await
        .map_err(|_| Error::Panicked("settings file read"))??;
    engine.replace_settings(expected, settings).await
}

/// A settings file, read as the host reads its own.
fn read_settings(path: &Path) -> Result<Settings> {
    SettingsFile::new(path)
        .load()?
        .ok_or_else(|| Error::other("no such file"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Severity;
    use prysm_ports::SessionState;
    use prysm_runtime::Engine;
    use prysm_runtime::fakes::{self, CallLog, FakeOutputs, FakeSources, MemoryStore};
    use std::time::Duration;

    async fn start(store: MemoryStore) -> (EngineHandle, Engine, CallLog) {
        let log = CallLog::new();
        let mut registry = prysm_ports::Registry::default();
        registry.add_source(FakeSources(log.clone()));
        registry.add_output(FakeOutputs(log.clone()));
        let (handle, engine) = Engine::start(registry, Box::new(store), Duration::from_secs(2))
            .await
            .unwrap();
        (handle, engine, log)
    }

    async fn within<T>(future: impl Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(5), future)
            .await
            .expect("timed out")
    }

    #[tokio::test]
    async fn targets_drive_the_engine_and_failures_are_error_notices() {
        within(async {
            let (handle, engine, log) = start(MemoryStore::with(fakes::settings())).await;
            let on = execute(
                handle.clone(),
                Command::SetTarget(LightingTarget::Processed),
            )
            .await;
            assert_eq!(on, Notice::info("Target processed"));
            assert_eq!(handle.snapshot().borrow().session, SessionState::Running);
            assert!(log.wait_for("output.write:", 1).await);
            let off = execute(handle.clone(), Command::SetTarget(LightingTarget::Off)).await;
            assert_eq!(off, Notice::info("Target off"));
            assert_eq!(handle.snapshot().borrow().session, SessionState::Stopped);
            engine.shutdown().await.unwrap();

            // Nothing configured: the engine's validation error is shown.
            let (handle, engine, _) = start(MemoryStore::new()).await;
            let failed = execute(handle, Command::SetTarget(LightingTarget::Processed)).await;
            assert_eq!(failed.severity, Severity::Error);
            assert!(failed.text.contains("not configured"), "{}", failed.text);
            engine.shutdown().await.unwrap();
        })
        .await;
    }

    #[tokio::test]
    async fn load_replaces_settings_at_the_shown_revision() {
        within(async {
            let store = MemoryStore::new();
            let (handle, engine, _) = start(store.clone()).await;
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("settings.json");
            std::fs::write(
                &path,
                r#"{ "format": 1, "capture": { "kind": "fake" }, "output": { "kind": "fake" } }"#,
            )
            .unwrap();
            let next = handle.snapshot().borrow().revision.next();
            let loaded = execute(handle.clone(), Command::Load(path.clone())).await;
            assert_eq!(loaded.severity, Severity::Info, "{}", loaded.text);
            assert!(
                loaded
                    .text
                    .starts_with(&format!("Settings revision {next}"))
            );
            assert_eq!(handle.snapshot().borrow().revision, next);
            assert_eq!(store.saved().unwrap().output.unwrap().kind, "fake");

            std::fs::write(&path, r#"{ "format": 1, "output": { "kind": "laser" } }"#).unwrap();
            let rejected = execute(handle.clone(), Command::Load(path.clone())).await;
            assert_eq!(rejected.severity, Severity::Error);
            assert!(
                rejected.text.contains("unknown kind `laser`"),
                "{}",
                rejected.text
            );

            std::fs::write(&path, "{ not json").unwrap();
            let invalid = execute(handle.clone(), Command::Load(path.clone())).await;
            assert!(
                invalid.text.contains("cannot parse settings file"),
                "{}",
                invalid.text
            );

            // A file in another format, as the host's own store refuses it.
            std::fs::write(&path, r#"{ "output": { "kind": "fake" } }"#).unwrap();
            let unversioned = execute(handle.clone(), Command::Load(path)).await;
            assert!(
                unversioned.text.contains("no `format` field"),
                "{}",
                unversioned.text
            );

            let missing = execute(handle.clone(), Command::Load(dir.path().join("none"))).await;
            assert!(missing.text.ends_with("no such file"), "{}", missing.text);
            assert_eq!(handle.snapshot().borrow().revision, next);
            engine.shutdown().await.unwrap();
        })
        .await;
    }
}
