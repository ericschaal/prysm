use super::*;
use futures::{FutureExt, StreamExt, poll};
use prysm_contract::{
    capture::CapturedFrame,
    identity::{FrameStamp, SettingsRevision, StreamEpoch},
    observation::Update,
    output::{ColorFrame, OfferResult},
    preview::PreviewSource,
};
use prysm_core::{BrightnessPercent, Config, Frame, PixelFormat, SmoothingSeconds};
use std::{
    num::NonZeroU64,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{sync::watch, task::JoinHandle};

const INSTANCE: InstanceId = InstanceId(NonZeroU64::MIN);

async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("flow made no progress")
}

fn session(sequence: u64) -> SessionId {
    SessionId {
        instance: INSTANCE,
        sequence: NonZeroU64::new(sequence).unwrap(),
    }
}

fn frame(session_number: u64, generation: u64, sequence: u64, value: u8) -> CapturedFrame {
    CapturedFrame {
        stamp: FrameStamp {
            epoch: StreamEpoch {
                session: session(session_number),
                generation,
            },
            sequence,
        },
        frame: Frame::fill(value, 8, 4, PixelFormat::RGB24).unwrap(),
    }
}

fn config() -> Config {
    Config {
        smoothing_seconds: SmoothingSeconds(0.0),
        remove_black_bars: false,
        ..Config::default()
    }
}

struct ReadGuard {
    cancelled: watch::Sender<usize>,
    complete: bool,
}
impl Drop for ReadGuard {
    fn drop(&mut self) {
        if !self.complete {
            self.cancelled.send_modify(|count| *count += 1);
        }
    }
}

struct Source {
    frames: mpsc::Receiver<Result<CapturedFrame, ControlError>>,
    reads: watch::Sender<usize>,
    cancelled: watch::Sender<usize>,
}
impl FrameSource for Source {
    async fn next_frame(&mut self) -> Result<Option<CapturedFrame>, ControlError> {
        self.reads.send_modify(|count| *count += 1);
        let mut guard = ReadGuard {
            cancelled: self.cancelled.clone(),
            complete: false,
        };
        let result = self.frames.recv().await.transpose();
        guard.complete = true;
        result
    }
}

struct Sink {
    frames: mpsc::UnboundedSender<ColorFrame>,
    // Test-only pause at the worker's output seam, never a production blocking sink.
    gate: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    dropped: Arc<AtomicBool>,
}
impl ColorSink for Sink {
    fn offer(&self, frame: ColorFrame) -> Result<OfferResult, ControlError> {
        self.frames.send(frame).unwrap();
        if let Some(gate) = self.gate.lock().unwrap().take() {
            let _ = gate.recv();
        }
        Ok(OfferResult::Queued)
    }
}
impl Drop for Sink {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}
fn sink(
    gate: Option<std::sync::mpsc::Receiver<()>>,
) -> (Sink, mpsc::UnboundedReceiver<ColorFrame>, Arc<AtomicBool>) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let dropped = Arc::new(AtomicBool::new(false));
    (
        Sink {
            frames: sender,
            gate: Mutex::new(gate),
            dropped: dropped.clone(),
        },
        receiver,
        dropped,
    )
}

struct Running {
    frames: mpsc::Sender<Result<CapturedFrame, ControlError>>,
    reads: watch::Receiver<usize>,
    cancelled: watch::Receiver<usize>,
    control: ProcessingHandle,
    preview: PreviewHandle,
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<Result<OwnerStopped, ControlError>>,
}
impl Running {
    async fn start(output: impl ColorSink) -> Self {
        let (sender, frames) = mpsc::channel(1);
        let (reads, mut read_count) = watch::channel(0);
        let (cancelled, cancellation_count) = watch::channel(0);
        let flow = ProcessingFlow::connect(
            Source {
                frames,
                reads,
                cancelled,
            },
            PrysmProcessor::default(),
            output,
            ProcessingExecution::DedicatedThread,
        );
        let (shutdown, stop) = oneshot::channel();
        let task = tokio::spawn(flow.owner.run(INSTANCE, async {
            let _ = stop.await;
        }));
        within(read_count.wait_for(|count| *count == 1))
            .await
            .unwrap();
        Self {
            frames: sender,
            reads: read_count,
            cancelled: cancellation_count,
            control: flow.control,
            preview: flow.preview,
            shutdown,
            task,
        }
    }
    async fn send(&mut self, frame: CapturedFrame) {
        let previous = *self.reads.borrow();
        self.frames.send(Ok(frame)).await.unwrap();
        // The next read begins only after the dispatcher has offered this frame.
        within(self.reads.wait_for(|count| *count > previous))
            .await
            .unwrap();
    }
    async fn configure(&self, revision: u64, config: Config) {
        let reply = within(self.control.configure(ProcessingUpdate {
            revision: SettingsRevision(revision),
            config,
        }))
        .await
        .unwrap();
        assert_eq!(reply.revision, SettingsRevision(revision));
    }
    async fn state(&self) -> ProcessingState {
        within(self.control.observe()).await.unwrap().initial.state
    }
    async fn stop(self) {
        self.shutdown.send(()).unwrap();
        assert_eq!(within(self.task).await.unwrap().unwrap(), OwnerStopped);
    }
}

#[tokio::test]
async fn commands_queued_before_worker_registration_are_processed() {
    let (_sender, frames) = mpsc::channel(1);
    let (sink, _, _) = sink(None);
    let flow = ProcessingFlow::connect(
        Source {
            frames,
            reads: watch::channel(0).0,
            cancelled: watch::channel(0).0,
        },
        PrysmProcessor::default(),
        sink,
        ProcessingExecution::DedicatedThread,
    );
    let mut configured = Box::pin(flow.control.configure(ProcessingUpdate {
        revision: SettingsRevision(7),
        config: config(),
    }));
    assert!(poll!(&mut configured).is_pending());

    let (shutdown, stop) = oneshot::channel();
    let task = tokio::spawn(flow.owner.run(INSTANCE, async {
        let _ = stop.await;
    }));
    assert_eq!(
        within(configured).await.unwrap().revision,
        SettingsRevision(7)
    );
    shutdown.send(()).unwrap();
    assert_eq!(within(task).await.unwrap().unwrap(), OwnerStopped);
}

#[tokio::test]
async fn idle_configuration_and_barriers_do_not_process_or_cancel_an_input_read() {
    let (sink, mut colors, _) = sink(None);
    let mut flow = Running::start(sink).await;
    assert_eq!(flow.state().await.configured_revision, None);
    flow.configure(7, config()).await;
    within(flow.control.barrier(session(1))).await.unwrap();
    let state = flow.state().await;
    assert_eq!(state.configured_revision, Some(SettingsRevision(7)));
    assert_eq!(state.applied_revision, None);
    assert_eq!(*flow.reads.borrow(), 1);
    assert_eq!(*flow.cancelled.borrow(), 0);
    assert!(colors.try_recv().is_err());

    flow.send(frame(2, 1, 1, 255)).await;
    let output = within(colors.recv()).await.unwrap();
    assert_eq!(output.revision, SettingsRevision(7));
    assert_eq!(output.colors.top.sample_at(0.5).r, 1.0);
    assert_eq!(
        flow.state().await.applied_revision,
        Some(SettingsRevision(7))
    );
    flow.configure(
        8,
        Config {
            brightness_percent: BrightnessPercent(0),
            ..config()
        },
    )
    .await;
    assert_eq!(
        flow.state().await.applied_revision,
        Some(SettingsRevision(7))
    );
    flow.send(frame(2, 1, 2, 255)).await;
    let output = within(colors.recv()).await.unwrap();
    assert_eq!(output.revision, SettingsRevision(8));
    assert_eq!(output.colors.top.sample_at(0.5).r, 0.0);
    assert_eq!(
        flow.state().await.applied_revision,
        Some(SettingsRevision(8))
    );
    flow.stop().await;
}

#[tokio::test]
async fn frames_wait_for_configuration_and_only_the_newest_survives() {
    let (sink, mut colors, _) = sink(None);
    let mut flow = Running::start(sink).await;
    for sequence in 1..=20 {
        flow.send(frame(1, 1, sequence, 255)).await;
    }
    assert!(colors.try_recv().is_err());
    assert_eq!(flow.state().await.applied_revision, None);
    flow.configure(3, config()).await;
    let output = within(colors.recv()).await.unwrap();
    assert_eq!(output.stamp.sequence, 20);
    assert_eq!(output.revision, SettingsRevision(3));
    within(flow.control.barrier(session(1))).await.unwrap();
    assert!(colors.try_recv().is_err());
    flow.stop().await;
}

#[tokio::test]
async fn slow_worker_keeps_one_latest_frame_and_configures_at_a_frame_boundary() {
    let (release, gate) = std::sync::mpsc::channel();
    let (sink, mut colors, _) = sink(Some(gate));
    let mut flow = Running::start(sink).await;
    flow.configure(1, config()).await;
    flow.send(frame(1, 1, 1, 255)).await;
    assert_eq!(
        within(colors.recv()).await.unwrap().revision,
        SettingsRevision(1)
    );
    for sequence in 2..=100 {
        flow.send(frame(1, 1, sequence, 255)).await;
    }
    // A delayed frame must not displace the pending newest frame.
    flow.send(frame(1, 1, 50, 255)).await;
    let control = flow.control.clone();
    let mut configured = Box::pin(control.configure(ProcessingUpdate {
        revision: SettingsRevision(2),
        config: Config {
            brightness_percent: BrightnessPercent(0),
            ..config()
        },
    }));
    assert!(poll!(&mut configured).is_pending());
    assert!(colors.try_recv().is_err());
    release.send(()).unwrap();
    assert_eq!(
        within(configured).await.unwrap().revision,
        SettingsRevision(2)
    );
    let output = within(colors.recv()).await.unwrap();
    assert_eq!(output.stamp.sequence, 100);
    assert_eq!(output.revision, SettingsRevision(2));
    assert_eq!(output.colors.top.sample_at(0.5).r, 0.0);
    within(flow.control.barrier(session(1))).await.unwrap();
    assert!(colors.try_recv().is_err());
    flow.stop().await;
}

#[tokio::test]
async fn barrier_waits_for_inflight_work_discards_pending_and_fences_late_sessions() {
    let (release, gate) = std::sync::mpsc::channel();
    let (sink, mut colors, _) = sink(Some(gate));
    let mut flow = Running::start(sink).await;
    flow.configure(1, config()).await;
    flow.send(frame(1, 1, 1, 255)).await;
    within(colors.recv()).await.unwrap();
    flow.send(frame(1, 1, 2, 255)).await;
    let control = flow.control.clone();
    let mut barrier = Box::pin(control.barrier(session(1)));
    assert!(poll!(&mut barrier).is_pending());
    flow.send(frame(1, 1, 3, 255)).await;
    release.send(()).unwrap();
    assert_eq!(within(barrier).await.unwrap().session, session(1));
    assert!(colors.try_recv().is_err());
    flow.send(frame(1, 99, 1000, 255)).await;
    flow.send(frame(2, 1, 1, 255)).await;
    assert_eq!(
        within(colors.recv()).await.unwrap().stamp.epoch.session,
        session(2)
    );
    within(flow.control.barrier(session(1))).await.unwrap();
    let mut foreign = session(2);
    foreign.instance = InstanceId(NonZeroU64::new(2).unwrap());
    assert!(matches!(
        within(flow.control.barrier(foreign)).await,
        Err(ControlError::Conflict(_))
    ));
    let mut wrong_instance = frame(3, 1, 1, 255);
    wrong_instance.stamp.epoch.session.instance = foreign.instance;
    flow.send(wrong_instance).await;
    flow.send(frame(2, 0, 100, 255)).await;
    flow.send(frame(2, 1, 2, 255)).await;
    assert_eq!(
        within(colors.recv()).await.unwrap().stamp,
        frame(2, 1, 2, 255).stamp
    );
    within(flow.control.barrier(session(2))).await.unwrap();
    assert!(colors.try_recv().is_err());
    flow.stop().await;
}

#[tokio::test]
async fn new_epochs_and_sessions_reset_history_but_configuration_does_not() {
    let (sink, mut colors, _) = sink(None);
    let mut flow = Running::start(sink).await;
    let config = Config {
        smoothing_seconds: SmoothingSeconds(1_000_000.0),
        ..config()
    };
    flow.configure(1, config).await;
    flow.send(frame(1, 1, 1, 0)).await;
    within(colors.recv()).await.unwrap();
    flow.configure(2, config).await;
    flow.send(frame(1, 1, 2, 255)).await;
    assert!(
        within(colors.recv())
            .await
            .unwrap()
            .colors
            .top
            .sample_at(0.5)
            .r
            < 0.01
    );
    flow.send(frame(1, 2, 1, 255)).await;
    assert_eq!(
        within(colors.recv())
            .await
            .unwrap()
            .colors
            .top
            .sample_at(0.5)
            .r,
        1.0
    );
    flow.send(frame(1, 1, 999, 0)).await;
    flow.send(frame(2, 1, 1, 0)).await;
    let output = within(colors.recv()).await.unwrap();
    assert_eq!(output.stamp.epoch.session, session(2));
    assert_eq!(output.colors.top.sample_at(0.5).r, 0.0);
    within(flow.control.barrier(session(2))).await.unwrap();
    assert!(colors.try_recv().is_err());
    flow.stop().await;
}

#[tokio::test]
async fn shutdown_waits_for_the_worker_drops_pending_and_closes_controls() {
    let (release, gate) = std::sync::mpsc::channel();
    let (sink, mut colors, dropped) = sink(Some(gate));
    let mut flow = Running::start(sink).await;
    flow.configure(1, config()).await;
    flow.send(frame(1, 1, 1, 255)).await;
    within(colors.recv()).await.unwrap();
    flow.send(frame(1, 1, 2, 255)).await;
    let control = flow.control.clone();
    let mut pending = Box::pin(control.configure(ProcessingUpdate {
        revision: SettingsRevision(2),
        config: config(),
    }));
    assert!(poll!(&mut pending).is_pending());
    flow.shutdown.send(()).unwrap();
    within(flow.cancelled.wait_for(|count| *count == 1))
        .await
        .unwrap();
    assert!((&mut flow.task).now_or_never().is_none());
    assert!(!dropped.load(Ordering::SeqCst));
    release.send(()).unwrap();
    assert_eq!(within(flow.task).await.unwrap().unwrap(), OwnerStopped);
    assert!(dropped.load(Ordering::SeqCst));
    assert!(colors.try_recv().is_err());
    assert!(matches!(
        within(pending).await,
        Err(ControlError::Unavailable(_))
    ));
    assert!(matches!(
        within(flow.control.barrier(session(1))).await,
        Err(ControlError::Unavailable(_))
    ));
}

#[tokio::test]
async fn observations_are_atomic_and_slow_subscribers_get_explicit_resync() {
    let (sink, _, _) = sink(None);
    let flow = Running::start(sink).await;
    flow.configure(1, config()).await;
    let mut observation = within(flow.control.observe()).await.unwrap();
    assert_eq!(observation.initial.cursor.instance, INSTANCE);
    assert_eq!(
        observation.initial.state.configured_revision,
        Some(SettingsRevision(1))
    );
    flow.configure(2, config()).await;
    let Update::Snapshot(next) = within(observation.updates.next()).await.unwrap().unwrap() else {
        panic!("snapshot expected")
    };
    assert_eq!(
        next.cursor.sequence,
        observation.initial.cursor.sequence + 1
    );
    assert_eq!(next.state.configured_revision, Some(SettingsRevision(2)));
    for revision in 3..(CAPACITY as u64 + 5) {
        flow.configure(revision, config()).await;
    }
    assert!(matches!(
        within(observation.updates.next()).await,
        Some(Ok(Update::ResyncRequired))
    ));
    assert!(within(observation.updates.next()).await.is_none());
    flow.stop().await;
}

#[tokio::test]
async fn source_closure_and_failure_end_the_owner_without_claiming_shutdown_success() {
    for fail in [false, true] {
        let (sink, _, dropped) = sink(None);
        let flow = Running::start(sink).await;
        let mut preview = flow.preview.subscribe().await.unwrap();
        if fail {
            flow.frames
                .send(Err(ControlError::Unavailable("source failed")))
                .await
                .unwrap();
        } else {
            drop(flow.frames);
        }
        let error = within(flow.task).await.unwrap().unwrap_err();
        assert!(error.to_string().contains(if fail {
            "source failed"
        } else {
            "capture frame owner closed"
        }));
        assert!(dropped.load(Ordering::SeqCst));
        assert!(within(preview.next()).await.is_none());
        assert!(flow.preview.subscribe().await.is_err());
    }
}

struct FaultSink {
    panic: bool,
}
impl ColorSink for FaultSink {
    fn offer(&self, _frame: ColorFrame) -> Result<OfferResult, ControlError> {
        assert!(!self.panic, "worker fault");
        Err(ControlError::Unavailable("output failed"))
    }
}

#[tokio::test]
async fn worker_failure_and_panic_are_joined_and_reported() {
    for panic in [false, true] {
        let flow = Running::start(FaultSink { panic }).await;
        let mut preview = flow.preview.subscribe().await.unwrap();
        flow.configure(1, config()).await;
        let mut observation = within(flow.control.observe()).await.unwrap();
        flow.frames.send(Ok(frame(1, 1, 1, 255))).await.unwrap();
        let error = within(flow.task).await.unwrap().unwrap_err();
        assert!(within(preview.next()).await.is_none());
        assert!(flow.preview.subscribe().await.is_err());
        assert!(error.to_string().contains(if panic {
            "worker fault"
        } else {
            "output failed"
        }));
        if !panic {
            let Update::Snapshot(failure) =
                within(observation.updates.next()).await.unwrap().unwrap()
            else {
                panic!("failure snapshot expected")
            };
            assert_eq!(failure.state.applied_revision, Some(SettingsRevision(1)));
            assert!(
                failure
                    .state
                    .failure
                    .unwrap()
                    .message
                    .contains("output failed")
            );
        }
        assert!(within(observation.updates.next()).await.is_none());
        assert!(matches!(
            within(flow.control.observe()).await,
            Err(ControlError::Unavailable(_))
        ));
    }
}

#[tokio::test]
async fn cancelling_the_owner_still_releases_the_worker() {
    let (sink, _, dropped) = sink(None);
    let flow = Running::start(sink).await;
    let mut preview = flow.preview.subscribe().await.unwrap();
    flow.configure(1, config()).await;
    flow.task.abort();
    assert!(within(flow.task).await.unwrap_err().is_cancelled());
    assert!(dropped.load(Ordering::SeqCst));
    assert!(within(preview.next()).await.is_none());
    assert!(flow.preview.subscribe().await.is_err());
    assert!(matches!(
        within(flow.control.observe()).await,
        Err(ControlError::Unavailable(_))
    ));
}

#[path = "tests/preview.rs"]
mod preview;

#[tokio::test]
async fn bounded_commands_remain_owned_after_the_callers_are_dropped() {
    let (release, gate) = std::sync::mpsc::channel();
    let (sink, mut colors, _) = sink(Some(gate));
    let mut flow = Running::start(sink).await;
    flow.configure(1, config()).await;
    flow.send(frame(1, 1, 1, 255)).await;
    within(colors.recv()).await.unwrap();
    for revision in 2..=(CAPACITY as u64 + 1) {
        assert!(
            flow.control
                .configure(ProcessingUpdate {
                    revision: SettingsRevision(revision),
                    config: config()
                })
                .now_or_never()
                .is_none()
        );
    }
    // This request has not entered the full mailbox, so dropping it cancels submission.
    assert!(
        flow.control
            .configure(ProcessingUpdate {
                revision: SettingsRevision(999),
                config: config()
            })
            .now_or_never()
            .is_none()
    );
    release.send(()).unwrap();
    assert_eq!(
        flow.state().await.configured_revision,
        Some(SettingsRevision(CAPACITY as u64 + 1))
    );
    flow.stop().await;
}

#[tokio::test]
async fn an_always_ready_source_yields_to_the_host() {
    struct ReadySource {
        produced: Arc<AtomicUsize>,
        started: Option<oneshot::Sender<()>>,
    }
    impl FrameSource for ReadySource {
        async fn next_frame(&mut self) -> Result<Option<CapturedFrame>, ControlError> {
            let sequence = self.produced.fetch_add(1, Ordering::SeqCst) + 1;
            if let Some(started) = self.started.take() {
                let _ = started.send(());
            }
            // Bound the test itself so a missing cooperative yield fails, rather than hangs.
            if sequence >= 10_000 {
                std::future::pending::<()>().await;
            }
            Ok(Some(frame(1, 1, sequence as u64, 255)))
        }
    }
    let produced = Arc::new(AtomicUsize::new(0));
    let (started, active) = oneshot::channel();
    let (sink, _, dropped) = sink(None);
    let flow = ProcessingFlow::connect(
        ReadySource {
            produced: produced.clone(),
            started: Some(started),
        },
        PrysmProcessor::default(),
        sink,
        ProcessingExecution::DedicatedThread,
    );
    let (shutdown, stop) = oneshot::channel();
    let task = tokio::spawn(flow.owner.run(INSTANCE, async {
        let _ = stop.await;
    }));
    within(active).await.unwrap();
    assert!(produced.load(Ordering::SeqCst) < 10_000);
    shutdown.send(()).unwrap();
    assert_eq!(within(task).await.unwrap().unwrap(), OwnerStopped);
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn dropping_control_clients_does_not_stop_processing() {
    let (sink, mut colors, _) = sink(None);
    let flow = Running::start(sink).await;
    flow.configure(1, config()).await;
    drop(flow.control);
    flow.frames.send(Ok(frame(1, 1, 1, 255))).await.unwrap();
    assert_eq!(
        within(colors.recv()).await.unwrap().revision,
        SettingsRevision(1)
    );
    flow.shutdown.send(()).unwrap();
    assert_eq!(within(flow.task).await.unwrap().unwrap(), OwnerStopped);
}
