//! Commands typed in the terminal, executed against the engine.
use crate::ReadSettings;
use crate::input::Notice;
use prysm_ports::{Error, LightingTarget, Result, chain};
use prysm_runtime::EngineHandle;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    SetTarget(LightingTarget),
    /// Replace all settings with a settings file, against the revision shown.
    Load(PathBuf),
}

/// Run one command and describe its outcome for the diagnostics panel.
/// `read` reads a settings file as the host reads its own.
pub(crate) async fn execute(engine: EngineHandle, read: ReadSettings, command: Command) -> Notice {
    match command {
        Command::SetTarget(target) => match engine.set_target(target).await {
            Ok(()) => Notice::info(format!("Target {}", target.as_str())),
            Err(error) => Notice::error(format!("Target {}: {}", target.as_str(), chain(&error))),
        },
        Command::Load(path) => {
            let shown = path.display().to_string();
            match load(&engine, read, path).await {
                Ok(revision) => Notice::info(format!("Settings revision {revision} from {shown}")),
                Err(error) => Notice::error(format!("Load {shown}: {}", chain(&error))),
            }
        }
    }
}

async fn load(
    engine: &EngineHandle,
    read: ReadSettings,
    path: PathBuf,
) -> Result<prysm_ports::Revision> {
    let expected = engine.snapshot().borrow().revision;
    let settings = tokio::task::spawn_blocking(move || read(&path))
        .await
        .map_err(|_| Error::Panicked("settings file read"))??;
    engine.replace_settings(expected, settings).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Severity;
    use prysm_ports::SessionState;
    use prysm_runtime::Engine;
    use prysm_runtime::fakes::{self, CallLog, FakeOutputs, FakeSources, MemoryStore};
    use prysm_testkit::within_async;
    use std::sync::Arc;
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

    /// A stand-in for the host's reader: the file's JSON as settings.
    fn read() -> ReadSettings {
        Arc::new(|path| {
            let bytes = std::fs::read(path).map_err(|e| Error::adapter("cannot read", e))?;
            serde_json::from_slice(&bytes).map_err(|e| Error::adapter("cannot parse", e))
        })
    }

    #[tokio::test]
    async fn targets_drive_the_engine_and_failures_are_error_notices() {
        within_async(Duration::from_secs(5), async {
            let (handle, engine, log) = start(MemoryStore::with(fakes::settings())).await;
            let target = |handle: &EngineHandle, target| {
                execute(handle.clone(), read(), Command::SetTarget(target))
            };
            let on = target(&handle, LightingTarget::Processed).await;
            assert_eq!(on, Notice::info("Target processed"));
            assert_eq!(handle.snapshot().borrow().session, SessionState::Running);
            assert!(log.wait_for("output.write:", 1).await);
            let off = target(&handle, LightingTarget::Off).await;
            assert_eq!(off, Notice::info("Target off"));
            assert_eq!(handle.snapshot().borrow().session, SessionState::Stopped);
            engine.shutdown().await.unwrap();

            // Nothing configured: the engine's validation error is shown.
            let (handle, engine, _) = start(MemoryStore::new()).await;
            let failed = target(&handle, LightingTarget::Processed).await;
            assert_eq!(failed.severity, Severity::Error);
            assert!(failed.text.contains("not configured"), "{}", failed.text);
            engine.shutdown().await.unwrap();
        })
        .await;
    }

    #[tokio::test]
    async fn load_replaces_settings_at_the_shown_revision() {
        within_async(Duration::from_secs(5), async {
            let store = MemoryStore::new();
            let (handle, engine, _) = start(store.clone()).await;
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("settings.json");
            std::fs::write(
                &path,
                r#"{ "capture": { "kind": "fake" }, "output": { "kind": "fake" } }"#,
            )
            .unwrap();
            let load = |path| execute(handle.clone(), read(), Command::Load(path));
            let next = handle.snapshot().borrow().revision.next();
            let loaded = load(path.clone()).await;
            assert_eq!(loaded.severity, Severity::Info, "{}", loaded.text);
            assert!(
                loaded
                    .text
                    .starts_with(&format!("Settings revision {next}"))
            );
            assert_eq!(handle.snapshot().borrow().revision, next);
            assert_eq!(store.saved().unwrap().output.unwrap().kind, "fake");

            std::fs::write(&path, r#"{ "output": { "kind": "laser" } }"#).unwrap();
            let rejected = load(path.clone()).await;
            assert_eq!(rejected.severity, Severity::Error);
            assert!(
                rejected.text.contains("unknown kind `laser`"),
                "{}",
                rejected.text
            );

            // The reader's failure is shown with its cause.
            std::fs::write(&path, "{ not json").unwrap();
            let invalid = load(path).await;
            assert_eq!(invalid.severity, Severity::Error);
            assert!(invalid.text.contains("cannot parse: "), "{}", invalid.text);
            assert_eq!(handle.snapshot().borrow().revision, next);
            engine.shutdown().await.unwrap();
        })
        .await;
    }
}
