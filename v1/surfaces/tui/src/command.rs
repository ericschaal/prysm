//! Commands typed in the terminal, executed against the engine.
use crate::input::Notice;
use prysm_ports::{Error, LightingTarget, Result, Settings};
use prysm_runtime::EngineHandle;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Settings files larger than this are rejected before parsing.
const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;

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
            Ok(()) => Notice::info(format!("Target {}", target_label(target))),
            Err(error) => Notice::error(format!(
                "Target {}: {}",
                target_label(target),
                describe(&error)
            )),
        },
        Command::Load(path) => {
            let shown = path.display().to_string();
            match load(&engine, path).await {
                Ok(revision) => Notice::info(format!("Settings revision {revision} from {shown}")),
                Err(error) => Notice::error(format!("Load {shown}: {}", describe(&error))),
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

fn read_settings(path: &Path) -> Result<Settings> {
    let file = std::fs::File::open(path).map_err(|e| Error::adapter("cannot open file", e))?;
    let mut bytes = Vec::new();
    file.take(MAX_SETTINGS_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::adapter("cannot read file", e))?;
    if bytes.len() as u64 > MAX_SETTINGS_BYTES {
        return Err(Error::other("settings file exceeds 1 MiB"));
    }
    serde_json::from_slice(&bytes).map_err(|e| Error::adapter("invalid settings file", e))
}

pub(crate) fn target_label(target: LightingTarget) -> &'static str {
    match target {
        LightingTarget::Off => "off",
        LightingTarget::Processed => "processed",
        LightingTarget::DeviceNative => "native",
    }
}

/// An error and its causes on one line.
pub(crate) fn describe(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
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
                r#"{ "capture": { "kind": "fake" }, "output": { "kind": "fake" } }"#,
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

            std::fs::write(&path, r#"{ "output": { "kind": "laser" } }"#).unwrap();
            let rejected = execute(handle.clone(), Command::Load(path.clone())).await;
            assert_eq!(rejected.severity, Severity::Error);
            assert!(
                rejected.text.contains("unknown kind `laser`"),
                "{}",
                rejected.text
            );

            std::fs::write(&path, "{ not json").unwrap();
            let invalid = execute(handle.clone(), Command::Load(path)).await;
            assert!(
                invalid.text.contains("invalid settings file"),
                "{}",
                invalid.text
            );

            let missing = execute(handle.clone(), Command::Load(dir.path().join("none"))).await;
            assert!(
                missing.text.contains("cannot open file"),
                "{}",
                missing.text
            );
            assert_eq!(handle.snapshot().borrow().revision, next);
            engine.shutdown().await.unwrap();
        })
        .await;
    }

    #[test]
    fn oversized_settings_files_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.json");
        let size = usize::try_from(MAX_SETTINGS_BYTES).unwrap() + 1;
        std::fs::write(&path, vec![b' '; size]).unwrap();
        let error = read_settings(&path).unwrap_err();
        assert!(error.to_string().contains("exceeds"));
    }
}
