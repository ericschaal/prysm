//! Gated persistence through public application ports; no filesystem or alternate state model.
use super::*;
use prysm_contract::{
    FailureKind,
    processing::{Configured, ProcessingState, ProcessingUpdate, Quiescent},
};

struct Save {
    value: TestSettings,
    reply: oneshot::Sender<Result<(), ControlError>>,
}
impl Save {
    fn ok(self) {
        self.reply.send(Ok(())).unwrap();
    }
    fn fail(self, error: ControlError) {
        self.reply.send(Err(error)).unwrap();
    }
}
struct GatedRepository {
    initial: Option<TestSettings>,
    saves: mpsc::UnboundedSender<Save>,
}
impl SettingsRepository<TestSettings> for GatedRepository {
    async fn load(&self) -> Result<Option<TestSettings>, ControlError> {
        Ok(self.initial.clone())
    }
    async fn save(&self, value: TestSettings) -> Result<(), ControlError> {
        let (reply, completion) = oneshot::channel();
        self.saves.send(Save { value, reply }).unwrap();
        completion.await.expect("accepted save remains owned")
    }
}
fn repository(initial: Option<TestSettings>) -> (GatedRepository, mpsc::UnboundedReceiver<Save>) {
    let (saves, receiver) = mpsc::unbounded_channel();
    (GatedRepository { initial, saves }, receiver)
}
fn replacement(
    request: u64,
    expected: u64,
    settings: TestSettings,
) -> Request<ApplicationCommand<TestSettings>> {
    Request {
        id: id(request),
        command: ApplicationCommand::ReplaceSettings {
            expected: SettingsRevision(expected),
            settings,
        },
    }
}
async fn replace(app: &Running, request: u64, expected: u64, settings: TestSettings) {
    within(app.control.submit(replacement(request, expected, settings)))
        .await
        .unwrap();
}
async fn save(saves: &mut mpsc::UnboundedReceiver<Save>) -> Save {
    within(saves.recv()).await.unwrap()
}
fn failed(status: OperationStatus, kind: FailureKind) {
    assert!(matches!(status, OperationStatus::Failed(failure) if failure.kind == kind));
}
async fn stop(app: &mut Running, request: u64) {
    app.submit(request, LightingTarget::Off).await;
    app.cleanup(0, true, IdleOutputMode::Off).await;
    assert!(matches!(
        app.result(request).await,
        OperationStatus::Succeeded
    ));
}

#[tokio::test]
async fn rejected_replacements_never_save_or_touch_devices() {
    let (repo, mut saves) = repository(Some(settings()));
    let mut app = Running::with_repository(settings(), repo).await;
    let mut invalid = settings();
    invalid.capture = Some(u32::MAX);
    let mut nonfinite = settings();
    nonfinite.processing.smoothing_seconds.0 = f32::NAN;
    for request in [
        replacement(1, 1, settings()),
        replacement(2, 0, invalid),
        replacement(3, 0, nonfinite),
    ] {
        assert!(app.control.submit(request.clone()).await.is_err());
        assert!(matches!(
            app.control.operation(request.id).await,
            Err(ControlError::UnknownOperation)
        ));
    }
    let mut foreign = replacement(4, 0, settings());
    foreign.id.instance = InstanceId(NonZeroU64::new(2).unwrap());
    assert!(app.control.submit(foreign).await.is_err());
    assert!(saves.try_recv().is_err());
    assert!(app.calls.try_recv().is_err());
    app.close().await;
}

#[tokio::test]
async fn save_is_owned_deduplicated_and_published_before_application() {
    let (repo, mut saves) = repository(Some(settings()));
    let mut app = Running::with_repository(settings(), repo).await;
    let mut value = settings();
    value.processing.brightness_percent.0 = 40;
    let request = replacement(1, 0, value.clone());
    app.control.submit(request.clone()).await.unwrap();
    let pending = save(&mut saves).await;
    assert_eq!(pending.value, value);
    assert!(matches!(
        app.control.operation(id(1)).await.unwrap(),
        OperationStatus::Pending
    ));
    assert_eq!(
        app.control
            .observe()
            .await
            .unwrap()
            .initial
            .state
            .saved_revision,
        SettingsRevision(0)
    );
    app.control.submit(request.clone()).await.unwrap();
    assert!(
        app.control
            .submit(replacement(1, 1, value.clone()))
            .await
            .is_err()
    );
    assert!(
        app.control
            .submit(replacement(2, 0, value.clone()))
            .await
            .is_err()
    );
    pending.ok();
    assert!(matches!(app.result(1).await, OperationStatus::Succeeded));
    let snapshot = app.control.observe().await.unwrap().initial.state;
    assert_eq!(snapshot.settings, value);
    assert_eq!(snapshot.saved_revision, SettingsRevision(1));
    assert_eq!(snapshot.configured_revision, None);
    assert_eq!(snapshot.applied_revision, None);
    app.control.submit(request).await.unwrap(); // expected zero is now old, but replay is valid.
    assert!(saves.try_recv().is_err());
    assert!(app.calls.try_recv().is_err());
    // A new ID still commits identical settings and increments the revision.
    replace(&app, 2, 1, value).await;
    save(&mut saves).await.ok();
    assert!(matches!(app.result(2).await, OperationStatus::Succeeded));
    assert_eq!(
        app.control
            .observe()
            .await
            .unwrap()
            .initial
            .state
            .saved_revision,
        SettingsRevision(2)
    );
    app.close().await;
}

#[tokio::test]
async fn missing_repository_still_saves_identical_defaults() {
    let (repo, mut saves) = repository(None);
    let mut defaults = settings();
    defaults.capture = None;
    defaults.output = None;
    let app = Running::with_repository(defaults.clone(), repo).await;
    replace(&app, 1, 0, defaults.clone()).await;
    let pending = save(&mut saves).await;
    assert_eq!(pending.value, defaults);
    pending.ok();
    assert!(matches!(app.result(1).await, OperationStatus::Succeeded));
    app.close().await;
}

#[tokio::test]
async fn failed_save_preserves_running_session_and_all_revisions() {
    let (repo, mut saves) = repository(Some(settings()));
    let mut app = Running::with_repository(settings(), repo).await;
    app.start().await;
    let before = app.state(|state| state.configured_revision.is_some()).await;
    let mut value = settings();
    value.output = Some(2);
    replace(&app, 2, 0, value).await;
    save(&mut saves)
        .await
        .fail(ControlError::Unavailable("disk full"));
    failed(app.result(2).await, FailureKind::Unavailable);
    let after = app.control.observe().await.unwrap().initial.state;
    assert_eq!(after.settings, before.settings);
    assert_eq!(after.saved_revision, before.saved_revision);
    assert_eq!(after.configured_revision, before.configured_revision);
    assert_eq!(after.applied_revision, before.applied_revision);
    assert!(matches!(after.session, SessionState::Waiting(s) if s == session()));
    assert!(app.calls.try_recv().is_err());
    stop(&mut app, 3).await;
    app.close().await;
}

#[tokio::test]
async fn processing_replacement_preserves_session_and_only_new_frames_advance_applied_revision() {
    let (repo, mut saves) = repository(Some(settings()));
    let mut app = Running::with_repository(settings(), repo).await;
    app.start().await;
    let mut value = settings();
    value.processing.brightness_percent.0 = 20;
    replace(&app, 2, 0, value.clone()).await;
    save(&mut saves).await.ok();
    assert!(matches!(app.result(2).await, OperationStatus::Succeeded));
    let state = app
        .state(|state| state.configured_revision == Some(SettingsRevision(1)))
        .await;
    assert_eq!(state.settings, value);
    assert_eq!(state.applied_revision, None);
    assert!(matches!(state.session, SessionState::Waiting(s) if s == session()));
    assert!(app.calls.try_recv().is_err());
    app.frames
        .send(CapturedFrame {
            stamp: FrameStamp {
                epoch: epoch(1),
                sequence: 1,
            },
            frame: Frame::dummy(4, 4).unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(
        within(app.colors.recv()).await.unwrap().revision,
        SettingsRevision(1)
    );
    app.state(|state| state.applied_revision == Some(SettingsRevision(1)))
        .await;
    stop(&mut app, 3).await;
    app.close().await;
}

#[tokio::test]
async fn off_finishes_while_save_is_pending_and_late_commit_never_restarts() {
    let (repo, mut saves) = repository(Some(settings()));
    let mut app = Running::with_repository(settings(), repo).await;
    app.start().await;
    let mut value = settings();
    value.capture = Some(7);
    replace(&app, 2, 0, value.clone()).await;
    let pending = save(&mut saves).await;
    stop(&mut app, 3).await;
    assert!(matches!(
        app.control.operation(id(2)).await.unwrap(),
        OperationStatus::Pending
    ));
    assert!(
        app.control
            .submit(Request {
                id: id(4),
                command: ApplicationCommand::SetTarget(LightingTarget::Processed)
            })
            .await
            .is_err()
    );
    // Completed Off entries may be evicted, but the older pending save must survive.
    for request in 5..75 {
        app.submit(request, LightingTarget::Off).await;
        assert!(matches!(
            app.result(request).await,
            OperationStatus::Succeeded
        ));
    }
    assert!(matches!(
        app.control.operation(id(2)).await.unwrap(),
        OperationStatus::Pending
    ));
    pending.ok();
    failed(app.result(2).await, FailureKind::Interrupted);
    let state = app.control.observe().await.unwrap().initial.state;
    assert_eq!(state.settings, value);
    assert_eq!(state.saved_revision, SettingsRevision(1));
    assert_eq!(state.desired, LightingTarget::Off);
    assert!(matches!(state.session, SessionState::Idle));
    assert!(app.calls.try_recv().is_err());
    app.close().await;
}

#[tokio::test]
async fn adapter_change_cleans_old_session_before_starting_new_settings() {
    let (repo, mut saves) = repository(Some(settings()));
    let mut app = Running::with_repository(settings(), repo).await;
    app.start().await;
    let mut value = settings();
    value.capture = Some(7);
    value.output = Some(9);
    replace(&app, 2, 0, value).await;
    save(&mut saves).await.ok();
    app.state(|state| state.saved_revision == SettingsRevision(1))
        .await;
    app.cleanup(0, true, IdleOutputMode::Off).await;
    let next = SessionId {
        sequence: NonZeroU64::new(2).unwrap(),
        ..session()
    };
    app.call(Call::ConfigureSettings(9)).await.ok();
    app.call(Call::Hold(StreamEpoch {
        session: next,
        generation: 0,
    }))
    .await
    .ok();
    app.call(Call::StartSettings(next, 7)).await.ok();
    assert!(matches!(app.result(2).await, OperationStatus::Succeeded));
    app.state(|state| state.configured_revision == Some(SettingsRevision(1)))
        .await;
    app.submit(3, LightingTarget::Off).await;
    app.call(Call::Hold(StreamEpoch {
        session: next,
        generation: 0,
    }))
    .await
    .ok();
    app.call(Call::Stop(next)).await.ok();
    app.call(Call::Finish(next, IdleOutputMode::Off)).await.ok();
    assert!(matches!(app.result(3).await, OperationStatus::Succeeded));
    app.close().await;
}

#[tokio::test]
async fn off_during_replacement_cleanup_attaches_without_restarting() {
    let (repo, mut saves) = repository(Some(settings()));
    let mut app = Running::with_repository(settings(), repo).await;
    app.start().await;
    let mut value = settings();
    value.output = Some(9);
    replace(&app, 2, 0, value).await;
    save(&mut saves).await.ok();
    let holding = app.call(Call::Hold(epoch(0))).await;
    app.submit(3, LightingTarget::Off).await;
    holding.ok();
    app.call(Call::Stop(session())).await.ok();
    app.call(Call::Finish(session(), IdleOutputMode::Off))
        .await
        .ok();
    failed(app.result(2).await, FailureKind::Interrupted);
    assert!(matches!(app.result(3).await, OperationStatus::Succeeded));
    assert!(app.calls.try_recv().is_err());
    app.close().await;
}

#[tokio::test]
async fn native_output_is_finished_on_old_device_before_new_device_is_configured() {
    let (repo, mut saves) = repository(Some(settings()));
    let mut app = Running::with_repository(settings(), repo).await;
    app.submit(1, LightingTarget::DeviceNative).await;
    app.call(Call::Configure).await.ok();
    app.call(Call::Finish(session(), IdleOutputMode::Native))
        .await
        .ok();
    assert!(matches!(app.result(1).await, OperationStatus::Succeeded));
    let mut value = settings();
    value.output = Some(9);
    replace(&app, 2, 0, value).await;
    save(&mut saves).await.ok();
    let old = SessionId {
        sequence: NonZeroU64::new(2).unwrap(),
        ..session()
    };
    let new = SessionId {
        sequence: NonZeroU64::new(3).unwrap(),
        ..session()
    };
    let finish = app.call(Call::Finish(old, IdleOutputMode::Off)).await;
    assert!(app.calls.try_recv().is_err());
    finish.ok();
    app.call(Call::ConfigureSettings(9)).await.ok();
    app.call(Call::Finish(new, IdleOutputMode::Native))
        .await
        .ok();
    assert!(matches!(app.result(2).await, OperationStatus::Succeeded));
    let shutdown_session = SessionId {
        sequence: NonZeroU64::new(4).unwrap(),
        ..session()
    };
    app.shutdown.send(()).unwrap();
    let finish = within(app.calls.recv()).await.unwrap();
    assert_eq!(
        finish.call,
        Call::Finish(shutdown_session, IdleOutputMode::Off)
    );
    finish.ok();
    within(app.app).await.unwrap().unwrap();
    app.flow_shutdown.send(()).unwrap();
    within(app.flow).await.unwrap().unwrap();
}

#[tokio::test]
async fn blocked_replacement_cleanup_preserves_saved_value_and_retry_start_uses_it() {
    let (repo, mut saves) = repository(Some(settings()));
    let mut app = Running::with_repository(settings(), repo).await;
    app.start().await;
    let mut value = settings();
    value.capture = Some(7);
    value.output = Some(9);
    replace(&app, 2, 0, value.clone()).await;
    save(&mut saves).await.ok();
    app.call(Call::Hold(epoch(0))).await.ok();
    app.call(Call::Stop(session()))
        .await
        .fail(ControlError::Unavailable("release failed"));
    failed(app.result(2).await, FailureKind::Unavailable);
    let snapshot = app.control.observe().await.unwrap().initial.state;
    assert_eq!(snapshot.settings, value);
    assert_eq!(snapshot.saved_revision, SettingsRevision(1));
    assert_eq!(snapshot.desired, LightingTarget::Off);
    assert!(matches!(
        snapshot.session,
        SessionState::Failed {
            cleanup_complete: false,
            ..
        }
    ));
    assert!(
        app.control
            .submit(replacement(3, 1, settings()))
            .await
            .is_err()
    );
    assert!(
        app.control
            .submit(Request {
                id: id(3),
                command: ApplicationCommand::SetTarget(LightingTarget::Processed)
            })
            .await
            .is_err()
    );
    stop(&mut app, 3).await;
    app.submit(4, LightingTarget::Processed).await;
    let next = SessionId {
        sequence: NonZeroU64::new(2).unwrap(),
        ..session()
    };
    app.call(Call::ConfigureSettings(9)).await.ok();
    app.call(Call::Hold(StreamEpoch {
        session: next,
        generation: 0,
    }))
    .await
    .ok();
    app.call(Call::StartSettings(next, 7)).await.ok();
    assert!(matches!(app.result(4).await, OperationStatus::Succeeded));
    app.state(|state| state.configured_revision == Some(SettingsRevision(1)))
        .await;
    app.submit(5, LightingTarget::Off).await;
    app.call(Call::Hold(StreamEpoch {
        session: next,
        generation: 0,
    }))
    .await
    .ok();
    app.call(Call::Stop(next)).await.ok();
    app.call(Call::Finish(next, IdleOutputMode::Off)).await.ok();
    app.result(5).await;
    app.close().await;
}

#[tokio::test]
async fn failed_cleanup_hold_reports_off_intent_and_does_not_restart() {
    let (repo, mut saves) = repository(Some(settings()));
    let mut app = Running::with_repository(settings(), repo).await;
    app.start().await;
    let mut value = settings();
    value.output = Some(9);
    replace(&app, 2, 0, value).await;
    save(&mut saves).await.ok();
    app.call(Call::Hold(epoch(0)))
        .await
        .fail(ControlError::Unavailable("black failed"));
    app.call(Call::Stop(session())).await.ok();
    app.call(Call::Finish(session(), IdleOutputMode::Off))
        .await
        .ok();
    failed(app.result(2).await, FailureKind::Unavailable);
    assert_eq!(
        app.control.observe().await.unwrap().initial.state.desired,
        LightingTarget::Off
    );
    assert!(app.calls.try_recv().is_err());
    app.close().await;
}

#[tokio::test]
async fn shutdown_waits_for_accepted_save_after_lighting_cleanup() {
    let (repo, mut saves) = repository(Some(settings()));
    let mut app = Running::with_repository(settings(), repo).await;
    app.start().await;
    replace(&app, 2, 0, settings()).await;
    let pending = save(&mut saves).await;
    app.shutdown.send(()).unwrap();
    for expected in [
        Call::Hold(epoch(0)),
        Call::Stop(session()),
        Call::Finish(session(), IdleOutputMode::Off),
    ] {
        let call = within(app.calls.recv()).await.unwrap();
        assert_eq!(call.call, expected);
        call.ok();
    }
    let state = within(app.control.observe()).await.unwrap().initial.state;
    assert_eq!(state.desired, LightingTarget::Off);
    assert!(!app.app.is_finished());
    assert!(!pending.reply.is_closed());
    pending.ok();
    within(app.app).await.unwrap().unwrap();
    assert!(app.calls.try_recv().is_err());
    app.flow_shutdown.send(()).unwrap();
    within(app.flow).await.unwrap().unwrap();
}

#[tokio::test]
async fn uncertain_save_retires_the_application_even_after_off_completed() {
    let (repo, mut saves) = repository(Some(settings()));
    let mut app = Running::with_repository(settings(), repo).await;
    app.start().await;
    replace(&app, 2, 0, settings()).await;
    let pending = save(&mut saves).await;
    stop(&mut app, 3).await;
    pending.fail(ControlError::RecoveryRequired(Box::new(
        std::io::Error::other("commit uncertain"),
    )));
    let error = within(app.app).await.unwrap().unwrap_err();
    assert_eq!(error.kind(), FailureKind::RecoveryRequired);
    app.flow_shutdown.send(()).unwrap();
    within(app.flow).await.unwrap().unwrap();
}

#[tokio::test]
async fn capture_failure_during_save_prevents_late_application() {
    let (repo, mut saves) = repository(Some(settings()));
    let mut app = Running::with_repository(settings(), repo).await;
    app.start().await;
    let mut value = settings();
    value.capture = Some(7);
    replace(&app, 2, 0, value).await;
    let pending = save(&mut saves).await;
    app.capture.set(CaptureState::Failed {
        session: session(),
        failure: prysm_contract::Failure::new(FailureKind::Unavailable, "capture lost"),
        cleanup_complete: true,
    });
    app.cleanup(0, true, IdleOutputMode::Off).await;
    pending.ok();
    failed(app.result(2).await, FailureKind::Unavailable);
    assert_eq!(
        app.control
            .observe()
            .await
            .unwrap()
            .initial
            .state
            .saved_revision,
        SettingsRevision(1)
    );
    assert!(app.calls.try_recv().is_err());
    app.close().await;
}

struct Configure {
    update: ProcessingUpdate,
    reply: oneshot::Sender<Result<(), ControlError>>,
}
#[derive(Clone)]
struct GatedProcessing {
    inner: ProcessingHandle,
    gate: Arc<Mutex<Option<oneshot::Sender<Configure>>>>,
}
impl ProcessingControl for GatedProcessing {
    async fn configure(&self, update: ProcessingUpdate) -> Result<Configured, ControlError> {
        let gate = if update.revision.0 > 0 {
            self.gate.lock().unwrap().take()
        } else {
            None
        };
        if let Some(gate) = gate {
            let (reply, done) = oneshot::channel();
            gate.send(Configure { update, reply })
                .ok()
                .expect("test retains configure gate");
            done.await.expect("configuration remains owned")?;
        }
        self.inner.configure(update).await
    }
    async fn barrier(&self, session: SessionId) -> Result<Quiescent, ControlError> {
        self.inner.barrier(session).await
    }
    async fn observe(&self) -> Result<Observation<ProcessingState>, ControlError> {
        self.inner.observe().await
    }
}

#[tokio::test]
async fn off_closes_admission_during_hot_configuration_but_barrier_waits_for_it() {
    let (repo, mut saves) = repository(Some(settings()));
    let (gate, update) = oneshot::channel();
    let mut app = Running::with_processing(settings(), repo, |inner| GatedProcessing {
        inner,
        gate: Arc::new(Mutex::new(Some(gate))),
    })
    .await;
    app.start().await;
    let mut value = settings();
    let correction = prysm_core::ColorCorrection::new(20, -10, 5).unwrap();
    let profile = prysm_contract::calibration::CalibrationProfile {
        id: prysm_contract::calibration::ProfileId(NonZeroU64::MIN),
        name: "active".into(),
        correction,
    };
    value.active_profile = Some(profile.id);
    value.profiles.push(profile);
    replace(&app, 2, 0, value).await;
    save(&mut saves).await.ok();
    let update = within(update).await.unwrap();
    assert_eq!(update.update.revision, SettingsRevision(1));
    assert_eq!(update.update.config.correction, correction);
    app.submit(3, LightingTarget::Off).await;
    app.call(Call::Hold(epoch(0))).await.ok();
    app.call(Call::Stop(session())).await.ok();
    // Observe after stop reply so the coordinator can process it without releasing configure.
    app.control.observe().await.unwrap();
    assert!(app.calls.try_recv().is_err());
    assert!(!update.reply.is_closed());
    assert!(matches!(
        app.control.operation(id(3)).await.unwrap(),
        OperationStatus::Pending
    ));
    update.reply.send(Ok(())).unwrap();
    app.call(Call::Finish(session(), IdleOutputMode::Off))
        .await
        .ok();
    failed(app.result(2).await, FailureKind::Interrupted);
    assert!(matches!(app.result(3).await, OperationStatus::Succeeded));
    app.close().await;
}

#[tokio::test]
async fn hot_configuration_failure_cleans_up_and_preserves_committed_settings() {
    let (repo, mut saves) = repository(Some(settings()));
    let (gate, update) = oneshot::channel();
    let mut app = Running::with_processing(settings(), repo, |inner| GatedProcessing {
        inner,
        gate: Arc::new(Mutex::new(Some(gate))),
    })
    .await;
    app.start().await;
    replace(&app, 2, 0, settings()).await;
    save(&mut saves).await.ok();
    within(update)
        .await
        .unwrap()
        .reply
        .send(Err(ControlError::Unavailable("configuration failed")))
        .unwrap();
    app.cleanup(0, true, IdleOutputMode::Off).await;
    failed(app.result(2).await, FailureKind::Unavailable);
    let snapshot = app.control.observe().await.unwrap().initial.state;
    assert_eq!(snapshot.saved_revision, SettingsRevision(1));
    assert_eq!(snapshot.configured_revision, Some(SettingsRevision(0)));
    assert_eq!(snapshot.desired, LightingTarget::Off);
    app.close().await;
}

#[tokio::test]
async fn off_before_first_hot_configuration_poll_never_issues_it() {
    let app = Running::new(settings()).await;
    let (calls, mut receiver) = mpsc::unbounded_channel();
    let (gate, mut update) = oneshot::channel();
    let mut lifecycle = crate::session::Lifecycle::new(
        INSTANCE,
        (
            Capture {
                calls: Calls(calls.clone()),
                observed: app.capture.clone(),
            },
            GatedProcessing {
                inner: app.processing.clone(),
                gate: Arc::new(Mutex::new(Some(gate))),
            },
            Output {
                calls: Calls(calls),
                observed: app.output.clone(),
            },
        ),
        settings(),
        EdgeColors::default(),
    );
    lifecycle.request(LightingTarget::Processed).unwrap();
    within(async {
        tokio::join!(
            async {
                loop {
                    if matches!(
                        lifecycle.progress().await.outcome,
                        crate::session::Outcome::Ready
                    ) {
                        break;
                    }
                }
            },
            async {
                for expected in [
                    Call::Configure,
                    Call::Hold(epoch(0)),
                    Call::Start(session()),
                ] {
                    let called = receiver.recv().await.unwrap();
                    assert_eq!(called.call, expected);
                    called.ok();
                }
            },
        );
    })
    .await;
    lifecycle.replace(settings(), SettingsRevision(1), LightingTarget::Processed);
    // The configure future exists, but has never been polled.
    assert!(lifecycle.stop().settings.unwrap().is_ok());
    within(async {
        tokio::join!(
            async {
                while !lifecycle.is_idle() {
                    lifecycle.progress().await;
                }
            },
            async {
                for expected in [
                    Call::Hold(epoch(0)),
                    Call::Stop(session()),
                    Call::Finish(session(), IdleOutputMode::Off),
                ] {
                    let called = receiver.recv().await.unwrap();
                    assert_eq!(called.call, expected);
                    called.ok();
                }
            },
        );
    })
    .await;
    assert!(matches!(
        update.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(
        app.processing
            .observe()
            .await
            .unwrap()
            .initial
            .state
            .configured_revision,
        Some(SettingsRevision(0))
    );
    app.close().await;
}

#[tokio::test]
async fn accepted_save_survives_all_client_handles_disappearing() {
    let (repo, mut saves) = repository(Some(settings()));
    let app = Running::with_repository(settings(), repo).await;
    replace(&app, 1, 0, settings()).await;
    let pending = save(&mut saves).await;
    drop(app.control);
    assert!(!pending.reply.is_closed());
    pending.ok();
    // Shutdown must settle the accepted save even without clients to receive its result.
    app.shutdown.send(()).unwrap();
    within(app.app).await.unwrap().unwrap();
    app.flow_shutdown.send(()).unwrap();
    within(app.flow).await.unwrap().unwrap();
}

#[tokio::test]
async fn loaded_adapter_values_and_duplicate_profiles_are_validated_before_initialization() {
    use prysm_contract::calibration::{CalibrationProfile, ProfileId};
    let mut app = Running::new(settings()).await;
    let mut bad_device = settings();
    bad_device.output = Some(u32::MAX);
    let mut bad_number = settings();
    bad_number.processing.smoothing_seconds.0 = f32::INFINITY;
    let mut duplicate = settings();
    let profile = CalibrationProfile {
        id: ProfileId(NonZeroU64::MIN),
        name: "profile".into(),
        correction: Default::default(),
    };
    duplicate.profiles = vec![profile.clone(), profile];
    for invalid in [bad_device, bad_number, duplicate] {
        let (repository, mut saves) = repository(Some(invalid.clone()));
        let application = Application::new(
            app.capture_control.clone(),
            app.processing.clone(),
            app.output_control.clone(),
            repository,
            policy::V1Policy,
        );
        let error = within(application.owner.run(INSTANCE, std::future::pending()))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), FailureKind::Invalid);
        assert!(saves.try_recv().is_err());
        assert!(
            app.control
                .submit(replacement(1, 0, invalid))
                .await
                .is_err()
        );
    }
    assert!(app.calls.try_recv().is_err());
    app.close().await;
}
