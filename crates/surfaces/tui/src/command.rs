//! Commands typed in the terminal, executed against the engine.
use crate::ReadSettings;
use crate::input::Notice;
use prysm_ports::{Error, LightingTarget, Result, chain};
use prysm_runtime::EngineHandle;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    SetTarget(LightingTarget),
    Power,
    /// Replace all settings with a settings file, against the revision shown.
    Load(PathBuf),
}

/// Run one command and describe its outcome for the diagnostics panel.
/// `read` reads a settings file as the host reads its own.
pub(crate) async fn execute(engine: EngineHandle, read: ReadSettings, command: Command) -> Notice {
    match command {
        Command::SetTarget(target) => set_target(&engine, target).await,
        Command::Power => match engine.press_power().await {
            Ok(()) => Notice::info("Power requested"),
            Err(error) => Notice::error(format!("Power: {}", chain(&error))),
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

async fn set_target(engine: &EngineHandle, target: LightingTarget) -> Notice {
    match engine.set_target(target).await {
        Ok(()) => Notice::info(format!("Target {}", target.as_str())),
        Err(error) => Notice::error(format!("Target {}: {}", target.as_str(), chain(&error))),
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
    use prysm_ports::{AdapterSettings, OutputFactory, SessionState, WaitingFor};
    use prysm_runtime::Engine;
    use prysm_runtime::fakes::{
        self, CallLog, FakeOutputSettings, FakeOutputs, FakeSources, MemoryStore,
    };
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
    async fn targets_are_accepted_and_the_snapshot_reports_the_outcome() {
        within_async(Duration::from_secs(5), async {
            let (handle, engine, log) = start(MemoryStore::with(fakes::settings())).await;
            let set = |target| execute(handle.clone(), read(), Command::SetTarget(target));
            let mut snapshots = handle.snapshot();

            let on = set(LightingTarget::ScreenSync).await;
            assert_eq!(on, Notice::info("Target screen_sync"));
            snapshots
                .wait_for(|snapshot| matches!(snapshot.session, SessionState::ScreenSync))
                .await
                .unwrap();
            assert!(log.wait_for("output.write:", 1).await);

            let off = set(LightingTarget::Off).await;
            assert_eq!(off, Notice::info("Target off"));
            snapshots
                .wait_for(|snapshot| {
                    matches!(snapshot.session, SessionState::Stopped { failure: None })
                        && !snapshot.control.busy
                })
                .await
                .unwrap();

            let on = execute(handle.clone(), read(), Command::Power).await;
            assert_eq!(on, Notice::info("Power requested"));
            snapshots
                .wait_for(|snapshot| matches!(snapshot.session, SessionState::ScreenSync))
                .await
                .unwrap();
            engine.shutdown().await.unwrap();

            let (handle, engine, _) = start(MemoryStore::new()).await;
            let notice = execute(
                handle.clone(),
                read(),
                Command::SetTarget(LightingTarget::ScreenSync),
            )
            .await;
            assert_eq!(notice, Notice::info("Target screen_sync"));
            let mut snapshots = handle.snapshot();
            let failed = snapshots
                .wait_for(|snapshot| snapshot.session.failure().is_some())
                .await
                .unwrap();
            let failure = failed.session.failure().unwrap();
            assert!(failure.to_string().contains("not configured"), "{failure}");
            drop(failed);
            engine.shutdown().await.unwrap();
        })
        .await;
    }

    #[tokio::test]
    async fn power_cancels_a_request_waiting_for_lights() {
        within_async(Duration::from_secs(5), async {
            let mut settings = fakes::settings();
            settings.output = Some(AdapterSettings::new(
                FakeOutputs::KIND,
                &FakeOutputSettings {
                    name: "waiting".into(),
                    unreachable_until: Some("device online".into()),
                    ..FakeOutputSettings::default()
                },
            ));
            let (handle, engine, log) = start(MemoryStore::with(settings)).await;
            let mut snapshots = handle.snapshot();
            let power = || execute(handle.clone(), read(), Command::Power);
            assert_eq!(power().await, Notice::info("Power requested"));
            snapshots
                .wait_for(|snapshot| snapshot.control.waiting == Some(WaitingFor::Output))
                .await
                .unwrap();
            assert!(log.wait_for("output.monitor", 1).await);

            assert_eq!(power().await, Notice::info("Power requested"));
            snapshots
                .wait_for(|snapshot| {
                    snapshot.target == LightingTarget::Off && snapshot.control.waiting.is_none()
                })
                .await
                .unwrap();
            log.record("device online");
            engine.shutdown().await.unwrap();
            assert_eq!(log.count("output.open:waiting"), 0);
            assert_eq!(log.count("output.idle:waiting:Native"), 0);
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
