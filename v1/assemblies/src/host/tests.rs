use super::lifecycle::DEADLINE;
use super::*;
use futures::StreamExt;
use futures::stream;
use prysm_application::{Application, policy::V1Policy};
use prysm_capture_device::DeviceCapture;
use prysm_contract::application::LightingTarget;
use prysm_contract::application::{ApplicationObservation, Request};
use prysm_contract::identity::OperationId;
use prysm_contract::{
    application::{OperationStatus, SessionState},
    calibration::{CalibrationCommand, CalibrationReply, CalibrationStatus},
    capture::CaptureCapabilities,
    observation::{Cursor, Observation, Versioned},
    output::OutputCapabilities,
};
use prysm_flow::{ProcessingExecution, ProcessingFlow};
use prysm_output_wled::WledOutput;
use prysm_processor::PrysmProcessor;
use std::num::NonZeroU64;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::sync::{Notify, mpsc, oneshot};

#[cfg(test)]
struct FixedSettings<S>(S);
#[cfg(test)]
impl<S: Clone + Send + Sync + 'static> prysm_contract::storage::SettingsRepository<S>
    for FixedSettings<S>
{
    async fn load(&self) -> Result<Option<S>, prysm_contract::ControlError> {
        Ok(Some(self.0.clone()))
    }
    async fn save(&self, _: S) -> Result<(), prysm_contract::ControlError> {
        Err(prysm_contract::ControlError::Unavailable(
            "headless settings are read-only",
        ))
    }
}

type TestSettings = prysm_contract::application::Settings<
    prysm_capture_device::DeviceCaptureSettings,
    prysm_output_wled::WledSettings,
>;

const INSTANCE: InstanceId = InstanceId(NonZeroU64::MIN);
fn settings() -> TestSettings {
    TestSettings {
        capture: None,
        output: None,
        processing: Default::default(),
        profiles: vec![],
        active_profile: None,
    }
}
async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(2), future)
        .await
        .expect("host did not progress")
}

#[derive(Clone)]
struct Control {
    initialized: Arc<Notify>,
    submitted: Arc<AtomicBool>,
}
impl ApplicationControl for Control {
    type Settings = TestSettings;
    async fn observe(&self) -> Result<ApplicationObservation<TestSettings>, ControlError> {
        self.initialized.notified().await;
        Ok(Observation {
            initial: snapshot(),
            updates: stream::pending().boxed(),
        })
    }
    async fn submit(
        &self,
        request: Request<ApplicationCommand<TestSettings>>,
    ) -> Result<Accepted, ControlError> {
        self.submitted.store(true, Ordering::SeqCst);
        Ok(Accepted {
            operation: request.id,
        })
    }
    async fn operation(&self, _: OperationId) -> Result<OperationStatus, ControlError> {
        Ok(OperationStatus::Succeeded)
    }
    async fn calibrate(
        &self,
        _: Request<CalibrationCommand>,
    ) -> Result<CalibrationReply, ControlError> {
        unreachable!()
    }
}

struct Gate {
    stopped: oneshot::Receiver<()>,
    release: oneshot::Sender<Result<OwnerStopped, ControlError>>,
}
fn gated_task() -> (Task, Gate) {
    let (stopped, requested) = oneshot::channel();
    let (release, done) = oneshot::channel();
    let task = Task::spawn(|stop| async move {
        stop.await;
        stopped.send(()).ok();
        done.await.unwrap()
    });
    (
        task,
        Gate {
            stopped: requested,
            release,
        },
    )
}
fn fixture() -> (Owners, Control, [Gate; 4]) {
    let (application, app_gate) = gated_task();
    let (capture, capture_gate) = gated_task();
    let (flow, flow_gate) = gated_task();
    let (output, output_gate) = gated_task();
    let wled = WledOutput::new(INSTANCE);
    let control = Control {
        initialized: Arc::new(Notify::new()),
        submitted: Arc::new(AtomicBool::new(false)),
    };
    (
        Owners {
            settings: None,
            application,
            capture,
            flow,
            output,
            output_shutdown: wled.shutdown.into(),
        },
        control,
        [app_gate, capture_gate, flow_gate, output_gate],
    )
}
fn events() -> (
    impl FnMut(HostEvent) + Send + 'static,
    mpsc::UnboundedReceiver<HostEvent>,
) {
    let (sender, events) = mpsc::unbounded_channel();
    (
        move |event| {
            sender.send(event).unwrap();
        },
        events,
    )
}
async fn ready(events: &mut mpsc::UnboundedReceiver<HostEvent>) {
    within(async { while !matches!(events.recv().await.unwrap(), HostEvent::Ready(_)) {} }).await;
}

#[tokio::test]
async fn command_entry_waits_for_initialization_and_normal_stop_joins_in_order() {
    let (owners, control, [mut app, mut capture, mut flow, mut output]) = fixture();
    let initialized = control.initialized.clone();
    let submitted = control.submitted.clone();
    let (stop, stopped) = oneshot::channel();
    let (report, mut events) = events();
    let (input, receiver) = mpsc::channel(1);
    input
        .send(Ok(ApplicationCommand::SetTarget(LightingTarget::Processed)))
        .await
        .unwrap();
    let commands = stream::unfold(receiver, |mut receiver| async {
        let command = receiver.recv().await?;
        Some((command, receiver))
    })
    .boxed();
    let host = tokio::spawn(run(
        owners,
        control,
        commands,
        async {
            stopped.await.unwrap();
        },
        report,
    ));
    tokio::task::yield_now().await;
    assert!(!submitted.load(Ordering::SeqCst));
    initialized.notify_one();
    ready(&mut events).await;
    within(async {
        while !submitted.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await;
    stop.send(()).unwrap();
    within(&mut app.stopped).await.unwrap();
    within(input.closed()).await; // Entry closes before owner cleanup completes.
    assert!(flow.stopped.try_recv().is_err());
    assert!(capture.stopped.try_recv().is_err());
    assert!(output.stopped.try_recv().is_err());
    app.release.send(Ok(OwnerStopped)).unwrap();
    within(&mut flow.stopped).await.unwrap();
    assert!(capture.stopped.try_recv().is_err());
    flow.release.send(Ok(OwnerStopped)).unwrap();
    within(&mut capture.stopped).await.unwrap();
    within(&mut output.stopped).await.unwrap();
    output.release.send(Ok(OwnerStopped)).unwrap();
    assert!(!host.is_finished());
    capture.release.send(Ok(OwnerStopped)).unwrap();
    within(host).await.unwrap().unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn ready_shutdown_wins_over_command_after_initialization() {
    let (owners, control, [mut app, mut capture, mut flow, mut output]) = fixture();
    control.initialized.notify_one();
    let submitted = control.submitted.clone();
    let consumed = Arc::new(AtomicBool::new(false));
    let command_consumed = consumed.clone();
    let (input, receiver) = mpsc::channel(1);
    let commands = stream::unfold(receiver, |mut receiver| async {
        let command = receiver.recv().await?;
        Some((command, receiver))
    })
    .inspect(move |_| command_consumed.store(true, Ordering::SeqCst))
    .boxed();
    let (stop, stopped) = oneshot::channel();
    let (report, mut events) = events();
    let host = tokio::spawn(run(
        owners,
        control,
        commands,
        async { stopped.await.unwrap() },
        report,
    ));
    ready(&mut events).await;

    // No await between sends: on this single-thread runtime both are ready at the host's next poll.
    input
        .try_send(Ok(ApplicationCommand::SetTarget(LightingTarget::Processed)))
        .unwrap();
    stop.send(()).unwrap();

    within(&mut app.stopped).await.unwrap();
    within(input.closed()).await;
    assert!(!consumed.load(Ordering::SeqCst));
    assert!(!submitted.load(Ordering::SeqCst));
    assert!(matches!(events.try_recv(), Ok(HostEvent::Stopping)));
    assert!(events.try_recv().is_err());

    app.release.send(Ok(OwnerStopped)).unwrap();
    within(&mut flow.stopped).await.unwrap();
    flow.release.send(Ok(OwnerStopped)).unwrap();
    within(&mut capture.stopped).await.unwrap();
    within(&mut output.stopped).await.unwrap();
    capture.release.send(Ok(OwnerStopped)).unwrap();
    output.release.send(Ok(OwnerStopped)).unwrap();
    within(host).await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn startup_timeout_stops_every_owner_retains_late_completion_and_failure() {
    let (owners, control, [_app, mut capture, mut flow, mut output]) = fixture();
    let (report, mut events) = events();
    let host = tokio::spawn(run(owners, control, stream::pending(), pending(), report));
    tokio::task::yield_now().await;
    tokio::time::advance(DEADLINE).await;
    // Automatic paused-time advancement also covers the case before the host's first poll.
    within(&mut capture.stopped).await.unwrap();
    within(&mut flow.stopped).await.unwrap();
    within(&mut output.stopped).await.unwrap();
    flow.release.send(Ok(OwnerStopped)).unwrap();
    output.release.send(Ok(OwnerStopped)).unwrap();
    tokio::time::advance(DEADLINE).await;
    within(async {
        while !matches!(
            events.recv().await.unwrap(),
            HostEvent::CleanupOverdue { .. }
        ) {}
    })
    .await;
    assert!(!host.is_finished());
    capture
        .release
        .send(Err(ControlError::RecoveryRequired(Box::new(
            std::io::Error::other("release failed"),
        ))))
        .unwrap();
    let errors = within(host).await.unwrap().unwrap_err().failures;
    assert!(errors.iter().any(|e| e.message.contains("deadline")));
    assert!(errors.iter().any(|e| e.message.contains("release failed")));
}

#[tokio::test]
async fn every_unexpected_owner_exit_retires_the_composition_even_when_successful() {
    for failed in ["application", "capture", "processing", "output"] {
        let (mut owners, control, gates) = fixture();
        control.initialized.notify_one();
        let slot = match failed {
            "application" => &mut owners.application,
            "capture" => &mut owners.capture,
            "processing" => &mut owners.flow,
            _ => &mut owners.output,
        };
        // Settle and join the fixture being replaced; it must not become a detached task.
        slot.request_stop();
        let mut gates: Vec<_> = gates.into_iter().map(Some).collect();
        let index = match failed {
            "application" => 0,
            "capture" => 1,
            "processing" => 2,
            _ => 3,
        };
        let mut replaced = gates[index].take().unwrap();
        within(&mut replaced.stopped).await.unwrap();
        replaced.release.send(Ok(OwnerStopped)).unwrap();
        within(slot.completion()).await.unwrap().unwrap();
        slot.take_completed();
        *slot = Task::spawn(|_| async { Ok(OwnerStopped) });
        let (report, _events) = events();
        let host = tokio::spawn(run(owners, control, stream::pending(), pending(), report));
        for (i, gate) in gates.into_iter().enumerate() {
            if let Some(mut gate) = gate {
                if i == 0 {
                    continue;
                } // The application is deliberately cancelled.
                within(&mut gate.stopped).await.unwrap();
                gate.release.send(Ok(OwnerStopped)).unwrap();
            }
        }
        let error = within(host).await.unwrap().unwrap_err();
        assert!(
            error
                .failures
                .iter()
                .any(|e| e.message.contains(failed) && e.message.contains("unexpected"))
        );
    }
}

#[tokio::test]
async fn real_idle_owners_start_and_close_without_source_closure_faults() {
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
        FixedSettings(settings()),
        V1Policy,
    );
    let owners = Owners {
        settings: None,
        output: Task::spawn(|stop| output.owner.run(stop)),
        capture: Task::spawn(|stop| capture.owner.run(stop)),
        flow: Task::spawn(|stop| flow.owner.run(INSTANCE, stop)),
        application: Task::spawn(|stop| application.owner.run(INSTANCE, stop)),
        output_shutdown: output.shutdown.into(),
    };
    let (stop, stopped) = oneshot::channel();
    let (report, mut events) = events();
    let host = tokio::spawn(run(
        owners,
        application.control,
        stream::pending(),
        async {
            let _ = stopped.await;
        },
        report,
    ));
    ready(&mut events).await;
    stop.send(()).unwrap();
    within(host).await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn coordinated_deadline_takes_over_from_stalled_application_or_flow() {
    for stall_flow in [false, true] {
        let (owners, control, [mut app, mut capture, mut flow, mut output]) = fixture();
        control.initialized.notify_one();
        let (stop, stopped) = oneshot::channel();
        let (report, mut events) = events();
        let host = tokio::spawn(run(
            owners,
            control,
            stream::pending(),
            async {
                let _ = stopped.await;
            },
            report,
        ));
        ready(&mut events).await;
        stop.send(()).unwrap();
        within(&mut app.stopped).await.unwrap();
        if stall_flow {
            app.release.send(Ok(OwnerStopped)).unwrap();
            within(&mut flow.stopped).await.unwrap();
        }
        assert!(capture.stopped.try_recv().is_err());
        assert!(output.stopped.try_recv().is_err());
        tokio::time::advance(DEADLINE).await;
        within(&mut capture.stopped).await.unwrap();
        within(&mut output.stopped).await.unwrap();
        if !stall_flow {
            within(&mut flow.stopped).await.unwrap();
        }
        capture.release.send(Ok(OwnerStopped)).unwrap();
        output.release.send(Ok(OwnerStopped)).unwrap();
        assert!(!host.is_finished());
        flow.release.send(Ok(OwnerStopped)).unwrap();
        let error = within(host).await.unwrap().unwrap_err();
        assert_eq!(error.failures.len(), 1);
        assert!(error.to_string().contains("coordinated shutdown deadline"));
    }
}

#[tokio::test]
async fn coordinator_panic_after_command_acceptance_stops_independent_owners() {
    let (mut owners, control, [mut app, mut capture, mut flow, mut output]) = fixture();
    owners.application.request_stop();
    within(&mut app.stopped).await.unwrap();
    app.release.send(Ok(OwnerStopped)).unwrap();
    within(owners.application.completion())
        .await
        .unwrap()
        .unwrap();
    owners.application.take_completed();
    let (crash, crashed) = oneshot::channel();
    owners.application = Task::spawn(|_| async {
        crashed.await.unwrap();
        panic!("coordinator crash");
    });
    control.initialized.notify_one();
    let (report, mut events) = events();
    let host = tokio::spawn(run(
        owners,
        control,
        stream::iter([Ok(ApplicationCommand::SetTarget(LightingTarget::Processed))]),
        pending(),
        report,
    ));
    ready(&mut events).await;
    within(async { while !matches!(events.recv().await.unwrap(), HostEvent::Accepted(_)) {} })
        .await;
    crash.send(()).unwrap();
    within(&mut capture.stopped).await.unwrap();
    within(&mut flow.stopped).await.unwrap();
    within(&mut output.stopped).await.unwrap();
    output.release.send(Ok(OwnerStopped)).unwrap();
    flow.release.send(Ok(OwnerStopped)).unwrap();
    assert!(!host.is_finished());
    capture.release.send(Ok(OwnerStopped)).unwrap();
    let error = within(host).await.unwrap().unwrap_err();
    assert!(error.to_string().contains("coordinator crash"));
}

#[tokio::test]
async fn shutdown_during_initialization_never_opens_command_entry() {
    let (owners, control, [mut app, mut capture, mut flow, mut output]) = fixture();
    let submitted = control.submitted.clone();
    let (report, mut events) = events();
    let host = tokio::spawn(run(
        owners,
        control,
        stream::iter([Ok(ApplicationCommand::SetTarget(LightingTarget::Processed))]),
        async {},
        report,
    ));
    within(&mut app.stopped).await.unwrap();
    app.release.send(Ok(OwnerStopped)).unwrap();
    within(&mut flow.stopped).await.unwrap();
    flow.release.send(Ok(OwnerStopped)).unwrap();
    within(&mut capture.stopped).await.unwrap();
    within(&mut output.stopped).await.unwrap();
    capture.release.send(Ok(OwnerStopped)).unwrap();
    output.release.send(Ok(OwnerStopped)).unwrap();
    within(host).await.unwrap().unwrap();
    assert!(!submitted.load(Ordering::SeqCst));
    assert!(matches!(events.try_recv(), Ok(HostEvent::Stopping)));
    assert!(events.try_recv().is_err());
}

#[cfg(feature = "headless")]
#[tokio::test]
async fn real_file_repository_loads_saves_and_joins_with_idle_owners() {
    use crate::host::SettingsOwner;
    use prysm_contract::{identity::SettingsRevision, storage::SettingsRepository};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let store = prysm_settings_file::FileSettings::new(path.clone()).unwrap();
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
        store.repository,
        V1Policy,
    );
    let control = application.control.clone();
    let owners = Owners {
        settings: Some(SettingsOwner {
            task: Task::spawn(|stop| store.owner.run(stop)),
            shutdown: store.shutdown,
        }),
        output: Task::spawn(|stop| output.owner.run(stop)),
        capture: Task::spawn(|stop| capture.owner.run(stop)),
        flow: Task::spawn(|stop| flow.owner.run(INSTANCE, stop)),
        application: Task::spawn(|stop| application.owner.run(INSTANCE, stop)),
        output_shutdown: output.shutdown.into(),
    };
    let (stop, stopped) = oneshot::channel();
    let (report, mut events) = events();
    let host = tokio::spawn(run(
        owners,
        application.control,
        stream::pending(),
        async {
            let _ = stopped.await;
        },
        report,
    ));
    ready(&mut events).await;
    assert!(!path.exists());
    let mut saved = settings();
    saved.processing.brightness_percent = prysm_core::BrightnessPercent(42);
    let id = OperationId {
        instance: INSTANCE,
        request: NonZeroU64::MIN,
    };
    control
        .submit(Request {
            id,
            command: ApplicationCommand::ReplaceSettings {
                expected: SettingsRevision(0),
                settings: saved.clone(),
            },
        })
        .await
        .unwrap();
    within(async {
        while matches!(
            control.operation(id).await.unwrap(),
            OperationStatus::Pending
        ) {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(matches!(
        control.operation(id).await.unwrap(),
        OperationStatus::Succeeded
    ));
    stop.send(()).unwrap();
    within(host).await.unwrap().unwrap();
    let restarted = prysm_settings_file::FileSettings::new(path).unwrap();
    assert_eq!(restarted.repository.load().await.unwrap(), Some(saved));
    restarted.shutdown.request();
    within(restarted.owner.run(pending())).await.unwrap();
}

#[cfg(feature = "headless")]
#[tokio::test]
async fn terminal_shutdown_fences_file_writes_and_joins_commit_after_application_abort() {
    use crate::host::SettingsOwner;
    use prysm_contract::storage::SettingsRepository;
    use std::sync::{Mutex, mpsc as sync};
    // Serialization executes inside the real retained file worker.
    #[derive(serde::Deserialize)]
    struct Gated {
        #[serde(skip)]
        gate: Mutex<Option<(oneshot::Sender<()>, sync::Receiver<bool>)>>,
    }
    impl serde::Serialize for Gated {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            if let Some((entered, release)) = self.gate.lock().unwrap().take() {
                entered.send(()).unwrap();
                if release.recv().unwrap() {
                    return Err(serde::ser::Error::custom("late settings write failure"));
                }
            }
            serializer.serialize_unit()
        }
    }
    for fail in [false, true] {
        let (mut owners, control, [mut app, mut capture, mut flow, mut output]) = fixture();
        owners.application.request_stop();
        within(&mut app.stopped).await.unwrap();
        app.release.send(Ok(OwnerStopped)).unwrap();
        within(owners.application.completion())
            .await
            .unwrap()
            .unwrap();
        owners.application.take_completed();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = prysm_settings_file::FileSettings::<Gated, ()>::new(path.clone()).unwrap();
        let repository = store.repository.clone();
        let (entered, started) = oneshot::channel();
        let (release, released) = sync::sync_channel(1);
        let saved = prysm_contract::application::Settings {
            capture: Some(Gated {
                gate: Mutex::new(Some((entered, released))),
            }),
            output: None,
            processing: Default::default(),
            profiles: vec![],
            active_profile: None,
        };
        owners.application = Task::spawn(|stop| async move {
            store.repository.save(saved).await?;
            stop.await;
            Ok(OwnerStopped)
        });
        owners.settings = Some(SettingsOwner {
            task: Task::spawn(|stop| store.owner.run(stop)),
            shutdown: store.shutdown,
        });
        // A device failure forces terminal shutdown while the application awaits save.
        owners.capture.request_stop();
        within(&mut capture.stopped).await.unwrap();
        control.initialized.notify_one();
        let (report, mut events) = events();
        let host = tokio::spawn(run(owners, control, stream::pending(), pending(), report));
        ready(&mut events).await;
        within(started).await.unwrap();
        capture
            .release
            .send(Err(ControlError::Unavailable("capture failed")))
            .unwrap();
        within(&mut flow.stopped).await.unwrap();
        within(&mut output.stopped).await.unwrap();
        flow.release.send(Ok(OwnerStopped)).unwrap();
        output.release.send(Ok(OwnerStopped)).unwrap();
        assert!(repository.load().await.is_err()); // Admission fenced before app abort.
        assert!(!host.is_finished());
        tokio::time::pause();
        tokio::time::advance(DEADLINE).await;
        let pending = within(async {
            loop {
                if let HostEvent::CleanupOverdue { pending } = events.recv().await.unwrap() {
                    break pending;
                }
            }
        })
        .await;
        assert_eq!(pending, vec!["settings"]);
        assert!(!host.is_finished());
        tokio::time::resume();
        release.send(fail).unwrap();
        let failures = within(host).await.unwrap().unwrap_err().failures;
        assert!(
            failures
                .iter()
                .any(|f| f.message.contains("capture failed"))
        );
        assert_eq!(
            failures
                .iter()
                .any(|f| f.message.contains("late settings write failure")),
            fail
        );
        assert_eq!(path.exists(), !fail);
    }
}

#[cfg(feature = "tui")]
mod terminal_commands;

fn snapshot() -> Versioned<ApplicationSnapshot<TestSettings>> {
    Versioned {
        cursor: Cursor {
            instance: INSTANCE,
            sequence: 0,
        },
        state: ApplicationSnapshot {
            instance: INSTANCE,
            settings: settings(),
            saved_revision: prysm_contract::identity::SettingsRevision(0),
            configured_revision: None,
            applied_revision: None,
            desired: LightingTarget::Off,
            session: SessionState::Idle,
            capture: CaptureCapabilities {
                supports_patterns: false,
            },
            output: OutputCapabilities { native_mode: true },
            observed_capture: None,
            observed_output: None,
            calibration: CalibrationStatus::Idle,
        },
    }
}

mod connection;
