use super::*;

const INSTANCE: InstanceId = InstanceId(NonZeroU64::MIN);
fn session() -> SessionId {
    SessionId {
        instance: INSTANCE,
        sequence: NonZeroU64::MIN,
    }
}
fn epoch(generation: u64) -> StreamEpoch {
    StreamEpoch {
        session: session(),
        generation,
    }
}
fn complete(model: &mut Model, action: Action) -> Outcome {
    assert_eq!(model.next_action(), Some(action));
    model.scheduled(action);
    model.completed(action, Ok(()))
}
fn prepare(model: &mut Model) {
    model.request(LightingTarget::Processed, true);
    complete(model, Action::ConfigureProcessing);
    complete(model, Action::ConfigureOutput);
    complete(model, Action::Prepare(epoch(0)));
    assert!(matches!(
        complete(model, Action::Open(session())),
        Outcome::Ready
    ));
}

#[test]
fn availability_reconciles_only_the_latest_epoch_after_acknowledgment() {
    let mut model = Model::new(INSTANCE);
    prepare(&mut model);
    assert!(matches!(model.status(), SessionState::Waiting(_)));
    model.capture_changed(&CaptureState::Available(epoch(1)));
    model.scheduled(Action::Admit(epoch(1)));
    model.capture_changed(&CaptureState::Waiting {
        epoch: epoch(2),
        reason: "no signal".into(),
    });
    model.completed(Action::Admit(epoch(1)), Ok(()));
    assert!(matches!(model.status(), SessionState::Waiting(_)));
    complete(&mut model, Action::Hold(epoch(2)));
    model.capture_changed(&CaptureState::Available(epoch(1)));
    assert_eq!(model.next_action(), None);
    model.capture_changed(&CaptureState::Available(epoch(3)));
    complete(&mut model, Action::Admit(epoch(3)));
    assert!(matches!(model.status(), SessionState::Running(_)));
}

#[test]
fn stop_at_each_preparation_boundary_cleans_only_the_resources_acquired() {
    let preparation = [
        Action::ConfigureProcessing,
        Action::ConfigureOutput,
        Action::Prepare(epoch(0)),
        Action::Open(session()),
    ];
    for boundary in 0..=preparation.len() {
        let mut model = Model::new(INSTANCE);
        model.request(LightingTarget::Processed, true);
        for action in &preparation[..boundary] {
            complete(&mut model, *action);
        }
        model.request(LightingTarget::Off, true);
        if boundary >= 2 {
            complete(&mut model, Action::CloseAdmission(epoch(0)));
        }
        if boundary == 4 {
            complete(&mut model, Action::StopCapture(session()));
        }
        let barrier = complete(&mut model, Action::Barrier(session()));
        if boundary >= 2 {
            assert!(matches!(barrier, Outcome::Changed));
            assert!(matches!(
                complete(&mut model, Action::Finish(session(), IdleOutputMode::Off)),
                Outcome::Stopped(None)
            ));
        } else {
            assert!(matches!(barrier, Outcome::Stopped(None)));
        }
        assert!(model.is_idle());
        assert_eq!(model.next_action(), None);
    }
}

#[test]
fn blocked_cleanup_has_no_action_until_an_explicit_retry() {
    let mut model = Model::new(INSTANCE);
    prepare(&mut model);
    model.request(LightingTarget::Off, true);
    complete(&mut model, Action::CloseAdmission(epoch(0)));
    assert!(matches!(
        model.completed(
            Action::StopCapture(session()),
            Err(ControlError::CleanupPending(session()))
        ),
        Outcome::Blocked(_)
    ));
    assert!(matches!(
        model.check_request(LightingTarget::Processed),
        Err(ControlError::CleanupPending(_))
    ));
    assert_eq!(model.next_action(), None);
    model.fail(Failure {
        message: "late failure snapshot".into(),
    });
    assert_eq!(model.next_action(), None);
    model.request(LightingTarget::Off, true);
    complete(&mut model, Action::CloseAdmission(epoch(0)));
    complete(&mut model, Action::StopCapture(session()));
    complete(&mut model, Action::Barrier(session()));
    assert!(matches!(
        complete(&mut model, Action::Finish(session(), IdleOutputMode::Off)),
        Outcome::Stopped(None)
    ));
}

#[test]
fn obsolete_native_finish_is_followed_by_off_before_stopped() {
    let mut model = Model::new(INSTANCE);
    model.request(LightingTarget::DeviceNative, true);
    complete(&mut model, Action::ConfigureOutput);
    let native = Action::Finish(session(), IdleOutputMode::Native);
    model.scheduled(native);
    model.stop(IdleOutputMode::Off, true);
    assert!(matches!(model.completed(native, Ok(())), Outcome::Changed));
    assert!(!model.is_idle());
    assert!(matches!(
        complete(&mut model, Action::Finish(session(), IdleOutputMode::Off)),
        Outcome::Stopped(None)
    ));
}

#[test]
fn native_during_unprepared_start_is_rejected_without_changing_the_start() {
    let mut model = Model::new(INSTANCE);
    model.request(LightingTarget::Processed, true);
    for stage in [Action::ConfigureProcessing, Action::ConfigureOutput] {
        assert_eq!(model.next_action(), Some(stage));
        assert!(matches!(
            model.check_request(LightingTarget::DeviceNative),
            Err(ControlError::Conflict(_))
        ));
        assert!(model.check_request(LightingTarget::Off).is_ok());
        complete(&mut model, stage);
    }
    assert!(model.check_request(LightingTarget::DeviceNative).is_ok());
}

#[test]
fn failed_cleanup_hold_keeps_the_requested_native_mode_and_reports_failure() {
    let mut model = Model::new(INSTANCE);
    prepare(&mut model);
    model.request(LightingTarget::DeviceNative, true);
    assert!(matches!(
        model.completed(
            Action::CloseAdmission(epoch(0)),
            Err(ControlError::Unavailable("black write failed"))
        ),
        Outcome::Changed
    ));
    complete(&mut model, Action::StopCapture(session()));
    complete(&mut model, Action::Barrier(session()));
    assert!(matches!(
        complete(
            &mut model,
            Action::Finish(session(), IdleOutputMode::Native)
        ),
        Outcome::Stopped(Some(_))
    ));
    assert!(matches!(
        model.status(),
        SessionState::Failed {
            cleanup_complete: true,
            ..
        }
    ));
    model.stop(IdleOutputMode::Off, true);
    let next = SessionId {
        sequence: NonZeroU64::new(2).unwrap(),
        ..session()
    };
    assert_eq!(
        model.next_action(),
        Some(Action::Finish(next, IdleOutputMode::Off))
    );
}
