use super::*;
use crate::server::owners::SettingsOwner;
use prysm_application::ApplicationHandle;
use prysm_contract::{FailureKind, identity::SettingsRevision, storage::SettingsRepository};
use prysm_protocol::settings::SettingsDocument;
use prysm_settings_file::FileSettingsHandle;

struct Host {
    control: ApplicationHandle<HeadlessSettings>,
    input: mpsc::Sender<String>,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<Result<OwnerStopped, HostError>>,
    events: mpsc::UnboundedReceiver<HostEvent>,
}
impl Host {
    async fn start(
        repository: impl SettingsRepository<HeadlessSettings>,
        store: SettingsOwner,
    ) -> Self {
        let capture = DeviceCapture::new(INSTANCE);
        let output = WledOutput::new(INSTANCE);
        let flow = ProcessingFlow::connect(
            capture.frames,
            PrysmProcessor::new(&Default::default()),
            output.colors,
            ProcessingExecution::DedicatedThread,
        );
        let application = Application::new(
            capture.control,
            flow.control,
            output.control,
            repository,
            V1Policy,
        );
        let owners = Owners {
            settings: Some(store),
            output: Task::spawn(|stop| output.owner.run(stop)),
            capture: Task::spawn(|stop| capture.owner.run(stop)),
            flow: Task::spawn(|stop| flow.owner.run(INSTANCE, stop)),
            application: Task::spawn(|stop| application.owner.run(INSTANCE, stop)),
            output_shutdown: output.shutdown.into(),
        };
        let control = application.control;
        let (input, receiver) = mpsc::channel(16);
        // A live startup-input stream must not starve terminal commands.
        let commands = stream::select(
            stream::pending(),
            prysm_control_terminal::commands(control.clone(), receiver),
        );
        let (stop, stopped) = oneshot::channel();
        let (report, mut events) = events();
        let task = tokio::spawn(run(
            owners,
            control.clone(),
            commands,
            async {
                let _ = stopped.await;
            },
            report,
        ));
        ready(&mut events).await;
        Self {
            control,
            input,
            stop,
            task,
            events,
        }
    }
    async fn accepted(&mut self) -> OperationId {
        within(async {
            loop {
                match self.events.recv().await.unwrap() {
                    HostEvent::Accepted(accepted) => return accepted.operation,
                    HostEvent::CommandRejected(failure) => {
                        panic!("unexpected rejection: {failure}")
                    }
                    _ => {}
                }
            }
        })
        .await
    }
    async fn result(&self, id: OperationId) -> OperationStatus {
        within(async {
            loop {
                let status = self.control.operation(id).await.unwrap();
                if !matches!(status, OperationStatus::Pending) {
                    return status;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
    }
    async fn close(self) {
        self.stop.send(()).unwrap();
        within(self.task).await.unwrap().unwrap();
    }
}

fn candidate(dir: &std::path::Path) -> (std::path::PathBuf, HeadlessSettings) {
    let mut value = settings();
    value.processing.brightness_percent = prysm_core::BrightnessPercent(42);
    let path = dir.join("candidate.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&SettingsDocument::try_from(value.clone()).unwrap()).unwrap(),
    )
    .unwrap();
    (path, value)
}

#[tokio::test]
async fn off_is_prepared_while_load_waits_for_its_snapshot() {
    let control = Control {
        initialized: Arc::new(Notify::new()),
        submitted: Arc::new(AtomicBool::new(false)),
    };
    let initialized = control.initialized.clone();
    let (input, receiver) = mpsc::channel(16);
    let mut commands = prysm_control_terminal::commands(control, receiver);
    let dir = tempfile::tempdir().unwrap();
    let (path, _) = candidate(dir.path());
    input
        .send(format!("load {}", path.display()))
        .await
        .unwrap();
    let next = commands.next();
    tokio::pin!(next);
    assert!(futures::poll!(&mut next).is_pending());
    input.send("off".into()).await.unwrap();
    assert!(matches!(
        within(next).await.unwrap().unwrap(),
        ApplicationCommand::SetTarget(LightingTarget::Off)
    ));
    initialized.notify_one();
    assert!(matches!(
        within(commands.next()).await.unwrap().unwrap(),
        ApplicationCommand::ReplaceSettings {
            expected: SettingsRevision(0),
            ..
        }
    ));
}

#[tokio::test]
async fn terminal_load_persists_reports_completion_and_reloads_on_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("saved.json");
    let store = prysm_settings_file::FileSettings::new(path.clone()).unwrap();
    let mut host = Host::start(
        store.repository,
        SettingsOwner {
            task: Task::spawn(|stop| store.owner.run(stop)),
            shutdown: store.shutdown,
        },
    )
    .await;
    let (candidate, value) = candidate(dir.path());
    host.input
        .send(format!("load {}", candidate.display()))
        .await
        .unwrap();
    let id = host.accepted().await;
    assert!(matches!(host.result(id).await, OperationStatus::Succeeded));
    let state = host.control.observe().await.unwrap().initial.state;
    assert_eq!(state.settings, value);
    assert_eq!(state.saved_revision, SettingsRevision(1));
    host.close().await;

    let store = prysm_settings_file::FileSettings::new(path).unwrap();
    let host = Host::start(
        store.repository,
        SettingsOwner {
            task: Task::spawn(|stop| store.owner.run(stop)),
            shutdown: store.shutdown,
        },
    )
    .await;
    let state = host.control.observe().await.unwrap().initial.state;
    assert_eq!(state.settings, value);
    assert_eq!(state.saved_revision, SettingsRevision(0));
    host.close().await;
}

struct GatedSave {
    file: FileSettingsHandle<HeadlessSettings>,
    entered: mpsc::Sender<oneshot::Sender<()>>,
}
impl SettingsRepository<HeadlessSettings> for GatedSave {
    async fn load(&self) -> Result<Option<HeadlessSettings>, ControlError> {
        self.file.load().await
    }
    async fn save(&self, value: HeadlessSettings) -> Result<(), ControlError> {
        let (release, released) = oneshot::channel();
        self.entered.send(release).await.unwrap();
        released.await.unwrap();
        self.file.save(value).await
    }
}

#[tokio::test]
async fn terminal_off_completes_during_save_and_disconnection_does_not_cancel_commit() {
    let dir = tempfile::tempdir().unwrap();
    let store = prysm_settings_file::FileSettings::new(dir.path().join("saved.json")).unwrap();
    let file = store.repository.clone();
    let (entered, mut saving) = mpsc::channel(1);
    let mut host = Host::start(
        GatedSave {
            file: store.repository,
            entered,
        },
        SettingsOwner {
            task: Task::spawn(|stop| store.owner.run(stop)),
            shutdown: store.shutdown,
        },
    )
    .await;
    let (candidate, value) = candidate(dir.path());
    host.input
        .send(format!("load {}", candidate.display()))
        .await
        .unwrap();
    let replacement = host.accepted().await;
    let release = within(saving.recv()).await.unwrap();
    host.input.send("off".into()).await.unwrap();
    let off = host.accepted().await;
    assert_ne!(replacement, off);
    assert!(matches!(host.result(off).await, OperationStatus::Succeeded));
    assert!(matches!(
        host.control.operation(replacement).await.unwrap(),
        OperationStatus::Pending
    ));
    // Losing terminal input cannot retract a replacement already owned by the application.
    let (unused, _) = mpsc::channel(1);
    drop(std::mem::replace(&mut host.input, unused));
    release.send(()).unwrap();
    assert!(
        matches!(host.result(replacement).await, OperationStatus::Failed(failure) if failure.kind == FailureKind::Interrupted)
    );
    let state = host.control.observe().await.unwrap().initial.state;
    assert_eq!(state.desired, LightingTarget::Off);
    assert!(matches!(state.session, SessionState::Idle));
    assert_eq!(state.settings, value);
    assert_eq!(file.load().await.unwrap(), Some(value));
    host.close().await;
}

#[tokio::test]
async fn malformed_and_invalid_candidates_are_rejected_without_changing_saved_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("saved.json");
    let store = prysm_settings_file::FileSettings::new(path.clone()).unwrap();
    let mut host = Host::start(
        store.repository,
        SettingsOwner {
            task: Task::spawn(|stop| store.owner.run(stop)),
            shutdown: store.shutdown,
        },
    )
    .await;
    let candidate = dir.path().join("bad.json");
    for content in [
        "{",
        r#"{"active_profile":1}"#,
        r#"{"output":{"unknown":true}}"#,
    ] {
        std::fs::write(&candidate, content).unwrap();
        host.input
            .send(format!("load {}", candidate.display()))
            .await
            .unwrap();
        within(async {
            loop {
                match host.events.recv().await.unwrap() {
                    HostEvent::CommandRejected(_) => break,
                    HostEvent::Accepted(_) => panic!("bad candidate accepted"),
                    _ => {}
                }
            }
        })
        .await;
        assert!(!path.exists());
        assert_eq!(
            host.control
                .observe()
                .await
                .unwrap()
                .initial
                .state
                .saved_revision,
            SettingsRevision(0)
        );
    }
    host.input.send("off".into()).await.unwrap();
    let off = host.accepted().await;
    assert!(matches!(host.result(off).await, OperationStatus::Succeeded));
    host.close().await;
}
