use super::*;
use backend::{Completion, Run};
use futures::{FutureExt, StreamExt};
use prysm_contract::{Failure, FailureKind, identity::StreamEpoch, observation::Update};
use prysm_core::{Frame, PixelFormat};
use std::{num::NonZeroU64, time::Duration};
use tokio::{sync::watch, task::JoinHandle};
use tokio_util::sync::CancellationToken;

mod integration;

const INSTANCE: InstanceId = InstanceId(NonZeroU64::MIN);
fn session(sequence: u64) -> SessionId {
    SessionId {
        instance: INSTANCE,
        sequence: NonZeroU64::new(sequence).unwrap(),
    }
}
fn settings() -> DeviceCaptureSettings {
    DeviceCaptureSettings::Device {
        path: "fixture-device".into(),
        resolution: CaptureResolution::new(8, 4).unwrap(),
        rate: NonZeroU32::new(30).unwrap(),
    }
}
fn frame(value: u8) -> Frame {
    Frame::fill(value, 8, 4, PixelFormat::RGB24).unwrap()
}
async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("capture made no progress")
}
struct Native {
    ready: Option<oneshot::Sender<()>>,
    frames: watch::Sender<Option<Frame>>,
    stop: CancellationToken,
    finished: oneshot::Sender<Completion>,
}
impl Native {
    fn ready(&mut self) {
        self.ready.take().unwrap().send(()).unwrap();
    }
    fn finish(self) {
        self.finished
            .send(Completion {
                failure: None,
                cleanup_complete: true,
            })
            .ok()
            .unwrap();
    }
    fn fail(self, cleanup_complete: bool) {
        self.finished
            .send(Completion {
                failure: Some(Failure::new(
                    if cleanup_complete {
                        FailureKind::Adapter
                    } else {
                        FailureKind::RecoveryRequired
                    },
                    "native failure",
                )),
                cleanup_complete,
            })
            .ok()
            .unwrap();
    }
}
fn factory() -> (
    impl FnMut(&DeviceCaptureSettings) -> Result<Run, ControlError> + Send,
    mpsc::UnboundedReceiver<Native>,
) {
    let (opened, receiver) = mpsc::unbounded_channel();
    (
        move |_: &DeviceCaptureSettings| {
            let (ready, readiness) = oneshot::channel();
            let (frames, receiver) = watch::channel(None);
            let (finished, completion) = oneshot::channel();
            let stop = CancellationToken::new();
            opened
                .send(Native {
                    ready: Some(ready),
                    frames,
                    stop: stop.clone(),
                    finished,
                })
                .ok()
                .unwrap();
            Ok(Run {
                ready: readiness,
                frames: receiver,
                stop,
                finished: async move { completion.await.unwrap() }.boxed(),
            })
        },
        receiver,
    )
}
struct Running {
    control: DeviceHandle,
    frames: DeviceFrames,
    opened: mpsc::UnboundedReceiver<Native>,
    shutdown: oneshot::Sender<()>,
    owner: JoinHandle<Result<OwnerStopped, ControlError>>,
}
impl Running {
    fn new() -> Self {
        let capture = DeviceCapture::new(INSTANCE);
        let (open, opened) = factory();
        let (shutdown, stopped) = oneshot::channel();
        let owner = tokio::spawn(owner::run(
            capture.owner,
            async {
                let _ = stopped.await;
            },
            open,
        ));
        Self {
            control: capture.control,
            frames: capture.frames,
            opened,
            shutdown,
            owner,
        }
    }
    fn start(&self, sequence: u64) -> JoinHandle<Result<CaptureStarted, ControlError>> {
        let control = self.control.clone();
        tokio::spawn(async move {
            control
                .start(CaptureStart {
                    session: session(sequence),
                    settings: settings(),
                })
                .await
        })
    }
    fn stop(&self, sequence: u64) -> JoinHandle<Result<CaptureStopped, ControlError>> {
        let control = self.control.clone();
        tokio::spawn(async move { control.stop(session(sequence)).await })
    }
    async fn open(&mut self, sequence: u64) -> Native {
        let started = self.start(sequence);
        let mut native = within(self.opened.recv()).await.unwrap();
        native.ready();
        assert_eq!(
            within(started).await.unwrap().unwrap().session,
            session(sequence)
        );
        native
    }
    async fn stop_native(&self, native: Native, sequence: u64) {
        let stopped = self.stop(sequence);
        within(native.stop.cancelled()).await;
        native.finish();
        assert_eq!(
            within(stopped).await.unwrap().unwrap().session,
            session(sequence)
        );
    }
    async fn close(self) {
        self.shutdown.send(()).unwrap();
        assert_eq!(within(self.owner).await.unwrap().unwrap(), OwnerStopped);
    }
}
async fn state(control: &DeviceHandle, predicate: impl Fn(&CaptureState) -> bool) -> CaptureState {
    within(async {
        let mut observation = control.observe().await.unwrap();
        if predicate(&observation.initial.state) {
            return observation.initial.state;
        }
        while let Some(update) = observation.updates.next().await {
            if let Update::Snapshot(snapshot) = update.unwrap()
                && predicate(&snapshot.state)
            {
                return snapshot.state;
            }
        }
        panic!("observation ended")
    })
    .await
}

#[tokio::test]
async fn validation_and_foreign_commands_never_open_a_device() {
    let mut capture = Running::new();
    let bad = DeviceCaptureSettings::File {
        path: "file.mp4".into(),
        resolution: CaptureResolution::new(8, 4).unwrap(),
        rate: NonZeroU32::MIN,
    };
    assert!(capture.control.validate(&bad).is_err());
    assert!(
        capture
            .control
            .start(CaptureStart {
                session: session(1),
                settings: bad
            })
            .await
            .is_err()
    );
    let mut foreign = session(1);
    foreign.instance = InstanceId(NonZeroU64::new(2).unwrap());
    assert!(
        capture
            .control
            .start(CaptureStart {
                session: foreign,
                settings: settings()
            })
            .await
            .is_err()
    );
    assert!(capture.control.stop(foreign).await.is_err());
    assert!(capture.opened.try_recv().is_err());
    assert!(
        tokio::time::timeout(Duration::from_millis(5), capture.frames.next_frame())
            .await
            .is_err()
    );
    capture.close().await;
}

#[tokio::test]
async fn readiness_is_separate_from_frames_and_delivery_is_latest_only() {
    let mut capture = Running::new();
    let native = capture.open(1).await;
    assert!(matches!(
        capture.control.observe().await.unwrap().initial.state,
        CaptureState::Waiting {
            epoch: StreamEpoch { generation: 0, .. },
            ..
        }
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(5), capture.frames.next_frame())
            .await
            .is_err()
    );
    for value in 0..100 {
        native.frames.send(Some(frame(value))).unwrap();
    }
    let latest = within(capture.frames.next_frame()).await.unwrap().unwrap();
    assert_eq!(latest.frame.as_slice(), &[99; 96]);
    assert_eq!(
        latest.stamp.epoch,
        StreamEpoch {
            session: session(1),
            generation: 1
        }
    );
    capture.stop_native(native, 1).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(5), capture.frames.next_frame())
            .await
            .is_err()
    );
    capture.close().await;
}

#[tokio::test]
async fn stop_during_open_retains_cleanup_and_fences_late_readiness_and_frames() {
    let mut capture = Running::new();
    let started = capture.start(1);
    let mut native = within(capture.opened.recv()).await.unwrap();
    let stopped = capture.stop(1);
    within(native.stop.cancelled()).await;
    assert_eq!(
        within(started).await.unwrap().unwrap_err().kind(),
        FailureKind::Interrupted
    );
    assert!(!stopped.is_finished());
    assert!(matches!(
        capture.control.stop(session(1)).await,
        Err(ControlError::CleanupPending(_))
    ));
    assert!(within(capture.start(2)).await.unwrap().is_err());
    native.ready();
    native.frames.send(Some(frame(255))).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(5), capture.frames.next_frame())
            .await
            .is_err()
    );
    assert!(matches!(
        capture.control.observe().await.unwrap().initial.state,
        CaptureState::Stopping(_)
    ));
    native.finish();
    within(stopped).await.unwrap().unwrap();
    assert!(within(capture.start(1)).await.unwrap().is_err());
    let native = capture.open(2).await;
    capture.stop_native(native, 2).await;
    capture.close().await;
}

#[tokio::test(start_paused = true)]
async fn silence_and_resumption_advance_epochs_and_clear_pending_frames() {
    let mut capture = Running::new();
    let native = capture.open(1).await;
    native.frames.send(Some(frame(1))).unwrap();
    state(&capture.control, |s| {
        matches!(s, CaptureState::Available(_))
    })
    .await;
    tokio::time::advance(Duration::from_secs(2)).await;
    state(&capture.control, |s| {
        matches!(
            s,
            CaptureState::Waiting {
                epoch: StreamEpoch { generation: 2, .. },
                ..
            }
        )
    })
    .await;
    assert!(futures::poll!(Box::pin(capture.frames.next_frame())).is_pending());
    native.frames.send(Some(frame(0))).unwrap();
    let resumed = within(capture.frames.next_frame()).await.unwrap().unwrap();
    assert_eq!(resumed.stamp.epoch.generation, 3);
    assert_eq!(resumed.frame.as_slice(), &[0; 96]); // Black is valid capture content.
    capture.stop_native(native, 1).await;
    capture.close().await;
}

#[tokio::test]
async fn failed_open_with_proven_release_allows_a_fresh_session() {
    let mut capture = Running::new();
    let started = capture.start(1);
    let native = within(capture.opened.recv()).await.unwrap();
    native.fail(true);
    assert_eq!(
        within(started).await.unwrap().unwrap_err().kind(),
        FailureKind::Adapter
    );
    assert!(matches!(
        capture.control.observe().await.unwrap().initial.state,
        CaptureState::Failed {
            cleanup_complete: true,
            ..
        }
    ));
    capture.control.stop(session(1)).await.unwrap();
    let native = capture.open(2).await;
    capture.stop_native(native, 2).await;
    capture.close().await;
}

#[tokio::test(start_paused = true)]
async fn overdue_silence_precedes_a_frame_queued_while_owner_was_not_polled() {
    let (mut open, mut opened) = factory();
    let run = open(&settings()).unwrap();
    let mut native = opened.recv().await.unwrap();
    let mailbox = Arc::new(Mailbox::new(INSTANCE));
    let (reply, started) = oneshot::channel();
    let mut active = crate::session::Session::new(
        session(1),
        NonZeroU32::new(30).unwrap(),
        run,
        mailbox.clone(),
        reply,
    );
    native.ready();
    assert!(active.progress().await.is_none());
    started.await.unwrap().unwrap();
    native.frames.send(Some(frame(1))).unwrap();
    assert!(active.progress().await.is_none());
    assert!(matches!(
        mailbox.observe().unwrap().initial.state,
        CaptureState::Available(StreamEpoch { generation: 1, .. })
    ));
    tokio::time::advance(Duration::from_secs(3)).await;
    native.frames.send(Some(frame(2))).unwrap();
    assert!(active.progress().await.is_none());
    assert!(matches!(
        mailbox.observe().unwrap().initial.state,
        CaptureState::Waiting {
            epoch: StreamEpoch { generation: 2, .. },
            ..
        }
    ));
    assert!(active.progress().await.is_none());
    assert_eq!(mailbox.next().await.unwrap().stamp.epoch.generation, 3);
    active.stop(None);
    native.finish();
    let completion = active.progress().await.unwrap();
    assert!(active.completed(completion).is_none());
}

#[tokio::test]
async fn closed_readiness_waits_for_the_worker_cause_and_still_accepts_stop() {
    for stop_before_completion in [false, true] {
        let (mut open, mut opened) = factory();
        let run = open(&settings()).unwrap();
        let mut native = opened.recv().await.unwrap();
        let mailbox = Arc::new(Mailbox::new(INSTANCE));
        let (reply, mut started) = oneshot::channel();
        let mut active = crate::session::Session::new(
            session(1),
            NonZeroU32::new(30).unwrap(),
            run,
            mailbox.clone(),
            reply,
        );
        drop(native.ready.take());
        native.frames.send(Some(frame(1))).unwrap();
        assert!(active.progress().await.is_none());
        assert_eq!(
            mailbox.observe().unwrap().initial.state,
            CaptureState::Starting(session(1))
        );
        assert!(futures::poll!(&mut started).is_pending());
        // Repeated cancelled polls must neither repoll the closed oneshot nor deliver frames.
        for _ in 0..2 {
            assert!(futures::poll!(Box::pin(active.progress())).is_pending());
        }
        assert!(futures::poll!(Box::pin(mailbox.next())).is_pending());

        let stopped = if stop_before_completion {
            let (reply, stopped) = oneshot::channel();
            active.stop(Some(reply));
            assert!(native.stop.is_cancelled());
            Some(stopped)
        } else {
            None
        };
        native.fail(true);
        let completion = active.progress().await.unwrap();
        assert!(active.completed(completion).is_none());
        assert_eq!(
            started.await.unwrap().unwrap_err().kind(),
            if stop_before_completion {
                FailureKind::Interrupted
            } else {
                FailureKind::Adapter
            }
        );
        if let Some(stopped) = stopped {
            assert_eq!(stopped.await.unwrap().unwrap().session, session(1));
        }
        let CaptureState::Failed {
            failure,
            cleanup_complete,
            ..
        } = mailbox.observe().unwrap().initial.state
        else {
            panic!("native failure must be preserved");
        };
        assert!(cleanup_complete);
        assert_eq!(failure.kind, FailureKind::Adapter);
    }
}

#[tokio::test(start_paused = true)]
async fn completion_precedes_queued_readiness_silence_and_frames() {
    for already_available in [false, true] {
        let (mut open, mut opened) = factory();
        let run = open(&settings()).unwrap();
        let mut native = opened.recv().await.unwrap();
        let mailbox = Arc::new(Mailbox::new(INSTANCE));
        let (reply, mut started) = oneshot::channel();
        let mut active = crate::session::Session::new(
            session(1),
            NonZeroU32::new(30).unwrap(),
            run,
            mailbox.clone(),
            reply,
        );
        native.ready();
        if already_available {
            assert!(active.progress().await.is_none());
            (&mut started).await.unwrap().unwrap();
            native.frames.send(Some(frame(1))).unwrap();
            assert!(active.progress().await.is_none());
            tokio::time::advance(Duration::from_secs(3)).await;
        }
        native.frames.send(Some(frame(2))).unwrap();
        let before = mailbox.observe().unwrap().initial;
        native.fail(true);
        let completion = active.progress().await.unwrap();
        let after = mailbox.observe().unwrap().initial;
        assert_eq!(after.cursor, before.cursor);
        assert_eq!(after.state, before.state);
        assert!(active.completed(completion).is_none());
        if !already_available {
            assert_eq!(
                started.await.unwrap().unwrap_err().kind(),
                FailureKind::Adapter
            );
        }
        assert!(futures::poll!(Box::pin(mailbox.next())).is_pending());
    }
}

#[tokio::test]
async fn unproven_release_blocks_restart_and_never_acknowledges_stop() {
    let mut capture = Running::new();
    let native = capture.open(1).await;
    let stopped = capture.stop(1);
    within(native.stop.cancelled()).await;
    native.fail(false);
    assert_eq!(
        within(stopped).await.unwrap().unwrap_err().kind(),
        FailureKind::RecoveryRequired
    );
    assert_eq!(
        within(capture.start(2)).await.unwrap().unwrap_err().kind(),
        FailureKind::RecoveryRequired
    );
    assert_eq!(
        capture.control.stop(session(1)).await.unwrap_err().kind(),
        FailureKind::RecoveryRequired
    );
    assert!(matches!(
        capture.control.observe().await.unwrap().initial.state,
        CaptureState::Failed {
            cleanup_complete: false,
            ..
        }
    ));
    capture.shutdown.send(()).unwrap();
    assert_eq!(
        within(capture.owner).await.unwrap().unwrap_err().kind(),
        FailureKind::RecoveryRequired
    );
    assert!(capture.frames.next_frame().await.unwrap().is_none());
}

#[tokio::test]
async fn caller_cancellation_does_not_abandon_open_or_cleanup() {
    let mut capture = Running::new();
    let started = capture.start(1);
    let mut native = within(capture.opened.recv()).await.unwrap();
    started.abort();
    native.ready();
    state(&capture.control, |s| {
        matches!(s, CaptureState::Waiting { .. })
    })
    .await;
    let stopped = capture.stop(1);
    within(native.stop.cancelled()).await;
    stopped.abort();
    native.finish();
    state(&capture.control, |s| matches!(s, CaptureState::Idle)).await;
    let native = capture.open(2).await;
    capture.stop_native(native, 2).await;
    capture.close().await;
}

#[tokio::test]
async fn shutdown_waits_for_open_cleanup_and_closes_ports_afterward() {
    let mut capture = Running::new();
    let started = capture.start(1);
    let native = within(capture.opened.recv()).await.unwrap();
    capture.shutdown.send(()).unwrap();
    within(native.stop.cancelled()).await;
    assert!(!capture.owner.is_finished());
    assert!(capture.control.observe().await.is_ok());
    within(started).await.unwrap().unwrap_err();
    native.finish();
    within(capture.owner).await.unwrap().unwrap();
    assert!(capture.frames.next_frame().await.unwrap().is_none());
    assert!(capture.control.observe().await.is_err());
}

#[tokio::test]
async fn observation_lag_requires_resync_and_owner_closure_ends_subscription() {
    let mut capture = Running::new();
    let mut observation = capture.control.observe().await.unwrap();
    for id in 1..=9 {
        let native = capture.open(id).await;
        capture.stop_native(native, id).await;
    }
    assert!(matches!(
        observation.updates.next().await.unwrap().unwrap(),
        Update::ResyncRequired
    ));
    assert!(observation.updates.next().await.is_none());
    let mut fresh = capture.control.observe().await.unwrap();
    assert_eq!(fresh.initial.state, CaptureState::Idle);
    capture.close().await;
    assert!(within(fresh.updates.next()).await.is_none());
}

#[tokio::test]
async fn idle_stop_seals_a_late_start_without_opening_anything() {
    let mut capture = Running::new();
    capture.control.stop(session(1)).await.unwrap();
    assert!(within(capture.start(1)).await.unwrap().is_err());
    assert!(capture.opened.try_recv().is_err());
    let native = capture.open(2).await;
    assert!(capture.control.stop(session(1)).await.is_err());
    capture.stop_native(native, 2).await;
    capture.close().await;
}

#[tokio::test]
async fn real_backend_missing_device_reports_failure_and_releases_before_retry() {
    let capture = DeviceCapture::new(INSTANCE);
    let (shutdown, stop) = oneshot::channel();
    let owner = tokio::spawn(capture.owner.run(async {
        let _ = stop.await;
    }));
    let mut settings = settings();
    if let DeviceCaptureSettings::Device { path, .. } = &mut settings {
        *path = "/dev/prysm-test-missing-device".into();
    }
    assert!(
        within(capture.control.start(CaptureStart {
            session: session(1),
            settings
        }))
        .await
        .is_err()
    );
    state(&capture.control, |s| {
        matches!(
            s,
            CaptureState::Failed {
                cleanup_complete: true,
                ..
            }
        )
    })
    .await;
    capture.control.stop(session(1)).await.unwrap();
    shutdown.send(()).unwrap();
    within(owner).await.unwrap().unwrap();
}
