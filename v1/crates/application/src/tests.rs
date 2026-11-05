use super::*;
use futures::StreamExt;
use prysm_contract::{
    application::{ApplicationSnapshot, LightingTarget, SessionState},
    capture::{
        CaptureCapabilities, CaptureStart, CaptureStarted, CaptureState, CaptureStopped,
        CapturedFrame, FrameSource,
    },
    identity::{FrameStamp, SessionId, SettingsRevision, StreamEpoch},
    observation::{Cursor, Observation, Update, Versioned},
    output::{
        ColorFrame, ColorSink, IdleOutputMode, OfferResult, OutputApplied, OutputCapabilities,
        OutputState,
    },
};
use prysm_core::{EdgeColors, Frame, PixelFormat};
use prysm_flow::{ProcessingExecution, ProcessingFlow, ProcessingHandle};
use prysm_processor::PrysmProcessor;
use std::{
    num::NonZeroU64,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::broadcast, task::JoinHandle};

const INSTANCE: InstanceId = InstanceId(NonZeroU64::MIN);
type TestSettings = Settings<(), ()>;
type Snapshot = ApplicationSnapshot<TestSettings>;

async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("application made no progress")
}
fn id(request: u64) -> OperationId {
    OperationId {
        instance: INSTANCE,
        request: NonZeroU64::new(request).unwrap(),
    }
}
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
fn settings() -> TestSettings {
    Settings {
        capture: Some(()),
        output: Some(()),
        processing: Default::default(),
        profiles: vec![],
        active_profile: None,
    }
}

#[derive(Clone)]
struct Observed<S> {
    snapshot: Arc<Mutex<Versioned<S>>>,
    updates: broadcast::Sender<Update<S, std::convert::Infallible>>,
}
impl<S: Clone + Send + 'static> Observed<S> {
    fn new(state: S) -> Self {
        Self {
            snapshot: Arc::new(Mutex::new(Versioned {
                cursor: Cursor {
                    instance: INSTANCE,
                    sequence: 0,
                },
                state,
            })),
            updates: broadcast::channel(16).0,
        }
    }
    fn set(&self, state: S) {
        let mut snapshot = self.snapshot.lock().unwrap();
        snapshot.cursor.sequence += 1;
        snapshot.state = state;
        let _ = self.updates.send(Update::Snapshot(snapshot.clone()));
    }
    fn observe(&self) -> Observation<S> {
        let snapshot = self.snapshot.lock().unwrap();
        Observation {
            initial: snapshot.clone(),
            updates: futures::stream::unfold(Some(self.updates.subscribe()), |receiver| async {
                let mut receiver = receiver?;
                match receiver.recv().await {
                    Ok(update) => Some((Ok(update), Some(receiver))),
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        Some((Ok(Update::ResyncRequired), None))
                    }
                    Err(broadcast::error::RecvError::Closed) => None,
                }
            })
            .boxed(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Call {
    Configure,
    Hold(StreamEpoch),
    Start(SessionId),
    Admit(StreamEpoch),
    Stop(SessionId),
    Finish(SessionId, IdleOutputMode),
}
struct Called {
    call: Call,
    reply: oneshot::Sender<Result<(), ControlError>>,
}
impl Called {
    fn ok(self) {
        self.reply.send(Ok(())).unwrap();
    }
    fn fail(self, error: ControlError) {
        self.reply.send(Err(error)).unwrap();
    }
}
#[derive(Clone)]
struct Calls(mpsc::UnboundedSender<Called>);
impl Calls {
    async fn call(&self, call: Call) -> Result<(), ControlError> {
        let (reply, result) = oneshot::channel();
        self.0.send(Called { call, reply }).unwrap();
        result.await.expect("test retained its operation")
    }
}
#[derive(Clone)]
struct Capture {
    calls: Calls,
    observed: Observed<CaptureState>,
}
impl CaptureControl for Capture {
    type Settings = ();
    fn capabilities(&self) -> CaptureCapabilities {
        CaptureCapabilities {
            supports_patterns: false,
        }
    }
    fn validate(&self, _: &()) -> Result<(), ControlError> {
        Ok(())
    }
    async fn start(&self, request: CaptureStart<()>) -> Result<CaptureStarted, ControlError> {
        self.calls.call(Call::Start(request.session)).await?;
        Ok(CaptureStarted {
            session: request.session,
        })
    }
    async fn stop(&self, session: SessionId) -> Result<CaptureStopped, ControlError> {
        self.calls.call(Call::Stop(session)).await?;
        Ok(CaptureStopped { session })
    }
    async fn observe(&self) -> Result<Observation<CaptureState>, ControlError> {
        Ok(self.observed.observe())
    }
}
#[derive(Clone)]
struct Output {
    calls: Calls,
    observed: Observed<OutputState>,
}
impl Output {
    async fn apply(&self, call: Call) -> Result<OutputApplied, ControlError> {
        self.calls.call(call).await?;
        let mut state = self.observed.snapshot.lock().unwrap().state.clone();
        state.confirmation = Some(OutputApplied::Sent);
        self.observed.set(state);
        Ok(OutputApplied::Sent)
    }
}
impl OutputControl for Output {
    type Settings = ();
    fn capabilities(&self) -> OutputCapabilities {
        OutputCapabilities { native_mode: true }
    }
    fn validate(&self, _: &()) -> Result<(), ControlError> {
        Ok(())
    }
    async fn configure(&self, _: ()) -> Result<OutputApplied, ControlError> {
        self.apply(Call::Configure).await
    }
    async fn admit(&self, epoch: StreamEpoch) -> Result<OutputApplied, ControlError> {
        self.apply(Call::Admit(epoch)).await
    }
    async fn hold(
        &self,
        epoch: StreamEpoch,
        colors: EdgeColors,
    ) -> Result<OutputApplied, ControlError> {
        assert_eq!(colors, EdgeColors::default());
        self.apply(Call::Hold(epoch)).await
    }
    async fn finish(
        &self,
        session: SessionId,
        mode: IdleOutputMode,
    ) -> Result<OutputApplied, ControlError> {
        self.apply(Call::Finish(session, mode)).await
    }
    async fn observe(&self) -> Result<Observation<OutputState>, ControlError> {
        Ok(self.observed.observe())
    }
}
struct Frames(mpsc::Receiver<CapturedFrame>);
impl FrameSource for Frames {
    async fn next_frame(&mut self) -> Result<Option<CapturedFrame>, ControlError> {
        Ok(self.0.recv().await)
    }
}
struct Colors {
    frames: mpsc::UnboundedSender<ColorFrame>,
    gate: Arc<Mutex<Option<std::sync::mpsc::Receiver<()>>>>,
}
impl ColorSink for Colors {
    fn offer(&self, frame: ColorFrame) -> Result<OfferResult, ControlError> {
        self.frames.send(frame).unwrap();
        if let Some(gate) = self.gate.lock().unwrap().take() {
            let _ = gate.recv();
        }
        Ok(OfferResult::Queued)
    }
}
struct Repository(TestSettings);
impl SettingsRepository<TestSettings> for Repository {
    async fn load(&self) -> Result<Option<TestSettings>, ControlError> {
        Ok(Some(self.0.clone()))
    }
    async fn save(&self, _: TestSettings) -> Result<(), ControlError> {
        panic!("this slice must not save settings")
    }
}

struct Running {
    control: ApplicationHandle<TestSettings>,
    processing: ProcessingHandle,
    calls: mpsc::UnboundedReceiver<Called>,
    capture: Observed<CaptureState>,
    output: Observed<OutputState>,
    frames: mpsc::Sender<CapturedFrame>,
    colors: mpsc::UnboundedReceiver<ColorFrame>,
    color_gate: Arc<Mutex<Option<std::sync::mpsc::Receiver<()>>>>,
    shutdown: oneshot::Sender<()>,
    flow_shutdown: oneshot::Sender<()>,
    app: JoinHandle<Result<OwnerStopped, ControlError>>,
    flow: JoinHandle<Result<OwnerStopped, ControlError>>,
}
impl Running {
    async fn new(settings: TestSettings) -> Self {
        let (calls, receiver) = mpsc::unbounded_channel();
        let calls = Calls(calls);
        let capture = Observed::new(CaptureState::Idle);
        let output = Observed::new(OutputState {
            admitted: None,
            held: false,
            idle_mode: None,
            confirmation: None,
            failure: None,
        });
        let (frames, source) = mpsc::channel(1);
        let (sink, colors) = mpsc::unbounded_channel();
        let color_gate = Arc::new(Mutex::new(None));
        let flow = ProcessingFlow::connect(
            Frames(source),
            PrysmProcessor::new(&settings.processing),
            Colors {
                frames: sink,
                gate: color_gate.clone(),
            },
            ProcessingExecution::DedicatedThread,
        );
        let application = Application::new(
            Capture {
                calls: calls.clone(),
                observed: capture.clone(),
            },
            flow.control.clone(),
            Output {
                calls,
                observed: output.clone(),
            },
            Repository(settings),
            policy::V1Policy,
        );
        let (shutdown, stop) = oneshot::channel();
        let (flow_shutdown, flow_stop) = oneshot::channel();
        let processing = flow.control;
        let flow = tokio::spawn(flow.owner.run(INSTANCE, async {
            let _ = flow_stop.await;
        }));
        let app = tokio::spawn(application.owner.run(INSTANCE, async {
            let _ = stop.await;
        }));
        let running = Self {
            control: application.control,
            processing,
            calls: receiver,
            capture,
            output,
            frames,
            colors,
            color_gate,
            shutdown,
            flow_shutdown,
            app,
            flow,
        };
        assert!(matches!(
            within(running.control.observe())
                .await
                .unwrap()
                .initial
                .state
                .session,
            SessionState::Idle
        ));
        running
    }
    async fn submit(&self, request: u64, target: LightingTarget) {
        assert_eq!(
            within(self.control.submit(Request {
                id: id(request),
                command: ApplicationCommand::SetTarget(target)
            }))
            .await
            .unwrap()
            .operation,
            id(request)
        );
    }
    async fn call(&mut self, expected: Call) -> Called {
        let called = within(self.calls.recv()).await.unwrap();
        assert_eq!(called.call, expected);
        called
    }
    async fn state(&self, matches: impl Fn(&Snapshot) -> bool) -> Snapshot {
        within(async {
            let mut observation = self.control.observe().await.unwrap();
            if matches(&observation.initial.state) {
                return observation.initial.state;
            }
            loop {
                match observation.updates.next().await.unwrap().unwrap() {
                    Update::Snapshot(snapshot) if matches(&snapshot.state) => {
                        return snapshot.state;
                    }
                    Update::ResyncRequired => panic!("test observer lagged"),
                    _ => {}
                }
            }
        })
        .await
    }
    async fn result(&self, request: u64) -> OperationStatus {
        within(async {
            let mut observation = self.control.observe().await.unwrap();
            let result = self.control.operation(id(request)).await.unwrap();
            if !matches!(result, OperationStatus::Pending) {
                return result;
            }
            loop {
                if let Update::Event {
                    event:
                        prysm_contract::application::ApplicationEvent::OperationChanged {
                            id: operation,
                            status,
                        },
                    ..
                } = observation.updates.next().await.unwrap().unwrap()
                    && operation == id(request)
                    && !matches!(status, OperationStatus::Pending)
                {
                    return status;
                }
            }
        })
        .await
    }
    async fn start(&mut self) {
        self.submit(1, LightingTarget::Processed).await;
        self.call(Call::Configure).await.ok();
        self.call(Call::Hold(epoch(0))).await.ok();
        self.call(Call::Start(session())).await.ok();
        assert!(matches!(self.result(1).await, OperationStatus::Succeeded));
    }
    async fn cleanup(&mut self, generation: u64, capture: bool, mode: IdleOutputMode) {
        self.call(Call::Hold(epoch(generation))).await.ok();
        if capture {
            self.call(Call::Stop(session())).await.ok();
        }
        self.call(Call::Finish(session(), mode)).await.ok();
    }
    async fn close(self) {
        let _ = self.shutdown.send(());
        assert_eq!(within(self.app).await.unwrap().unwrap(), OwnerStopped);
        self.flow_shutdown.send(()).unwrap();
        assert_eq!(within(self.flow).await.unwrap().unwrap(), OwnerStopped);
    }
}

#[tokio::test]
async fn start_wait_resume_stop_with_real_flow_and_distinct_observed_revisions() {
    let mut app = Running::new(settings()).await;
    app.start().await;
    let state = app
        .state(|s| matches!(s.session, SessionState::Waiting(_)) && s.configured_revision.is_some())
        .await;
    assert_eq!(state.saved_revision, SettingsRevision(0));
    assert_eq!(state.configured_revision, Some(SettingsRevision(0)));
    assert_eq!(state.applied_revision, None);
    assert_eq!(
        state.observed_output.unwrap().confirmation,
        Some(OutputApplied::Sent)
    );
    app.capture.set(CaptureState::Available(epoch(1)));
    app.call(Call::Admit(epoch(1))).await.ok();
    app.state(|s| matches!(s.session, SessionState::Running(_)))
        .await;
    app.frames
        .send(CapturedFrame {
            stamp: FrameStamp {
                epoch: epoch(1),
                sequence: 1,
            },
            frame: Frame::fill(200, 8, 4, PixelFormat::RGB24).unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(
        within(app.colors.recv()).await.unwrap().revision,
        SettingsRevision(0)
    );
    app.state(|s| s.applied_revision == Some(SettingsRevision(0)))
        .await;
    app.capture.set(CaptureState::Waiting {
        epoch: epoch(2),
        reason: "no signal".into(),
    });
    app.call(Call::Hold(epoch(2))).await.ok();
    app.state(|s| matches!(s.session, SessionState::Waiting(_)))
        .await;
    app.capture.set(CaptureState::Available(epoch(1)));
    app.capture.set(CaptureState::Available(epoch(3)));
    app.call(Call::Admit(epoch(3))).await.ok();
    app.submit(2, LightingTarget::DeviceNative).await;
    app.cleanup(3, true, IdleOutputMode::Native).await;
    assert!(matches!(app.result(2).await, OperationStatus::Succeeded));
    app.state(|s| matches!(s.session, SessionState::Idle)).await;
    let observed = app.processing.observe().await.unwrap().initial.state;
    assert_eq!(observed.applied_revision, Some(SettingsRevision(0)));
    // The barrier sealed this session, including input that arrives after Stop.
    app.frames
        .send(CapturedFrame {
            stamp: FrameStamp {
                epoch: epoch(3),
                sequence: 99,
            },
            frame: Frame::fill(255, 8, 4, PixelFormat::RGB24).unwrap(),
        })
        .await
        .unwrap();
    app.submit(3, LightingTarget::Off).await;
    app.call(Call::Configure).await.ok();
    app.call(Call::Finish(
        SessionId {
            sequence: NonZeroU64::new(2).unwrap(),
            ..session()
        },
        IdleOutputMode::Off,
    ))
    .await
    .ok();
    assert!(matches!(app.result(3).await, OperationStatus::Succeeded));
    app.close().await;
}

#[tokio::test]
async fn stop_retains_delayed_output_preparation_and_survives_caller_loss() {
    let mut app = Running::new(settings()).await;
    let (reply, accepted) = oneshot::channel();
    app.control
        .0
        .send(Command::Submit(
            Request {
                id: id(1),
                command: ApplicationCommand::SetTarget(LightingTarget::Processed),
            },
            reply,
        ))
        .await
        .unwrap();
    drop(accepted);
    let configure = app.call(Call::Configure).await;
    app.submit(2, LightingTarget::Off).await;
    app.state(|s| matches!(s.session, SessionState::Stopping(_)))
        .await;
    assert!(app.calls.try_recv().is_err());
    assert!(matches!(app.result(1).await, OperationStatus::Failed(_)));
    configure.ok();
    app.cleanup(0, false, IdleOutputMode::Off).await;
    assert!(matches!(app.result(2).await, OperationStatus::Succeeded));
    app.close().await;
}

#[tokio::test]
async fn stop_reaches_a_racing_open_and_retains_its_reply_before_restart() {
    let mut app = Running::new(settings()).await;
    app.submit(1, LightingTarget::Processed).await;
    app.call(Call::Configure).await.ok();
    app.call(Call::Hold(epoch(0))).await.ok();
    let opening = app.call(Call::Start(session())).await;
    app.submit(2, LightingTarget::Off).await;
    app.call(Call::Hold(epoch(0))).await.ok();
    app.call(Call::Stop(session())).await.ok();
    assert!(app.calls.try_recv().is_err());
    assert!(matches!(
        app.control.operation(id(2)).await.unwrap(),
        OperationStatus::Pending
    ));
    assert!(matches!(
        app.control
            .submit(Request {
                id: id(3),
                command: ApplicationCommand::SetTarget(LightingTarget::Processed)
            })
            .await,
        Err(ControlError::Conflict(_))
    ));
    opening.fail(ControlError::Unavailable("open cancelled by stop"));
    app.call(Call::Finish(session(), IdleOutputMode::Off))
        .await
        .ok();
    assert!(matches!(app.result(2).await, OperationStatus::Succeeded));
    app.close().await;
}

#[tokio::test]
async fn failed_capture_cleanup_blocks_restart_and_explicit_off_retries() {
    let mut app = Running::new(settings()).await;
    app.start().await;
    app.submit(2, LightingTarget::Off).await;
    app.call(Call::Hold(epoch(0))).await.ok();
    app.call(Call::Stop(session()))
        .await
        .fail(ControlError::CleanupPending(session()));
    assert!(matches!(app.result(2).await, OperationStatus::Failed(_)));
    app.state(|s| {
        matches!(
            s.session,
            SessionState::Failed {
                cleanup_complete: false,
                ..
            }
        )
    })
    .await;
    assert!(matches!(
        app.control
            .submit(Request {
                id: id(3),
                command: ApplicationCommand::SetTarget(LightingTarget::Processed)
            })
            .await,
        Err(ControlError::CleanupPending(_))
    ));
    assert!(app.calls.try_recv().is_err());
    app.submit(4, LightingTarget::Off).await;
    app.cleanup(0, true, IdleOutputMode::Off).await;
    assert!(matches!(app.result(4).await, OperationStatus::Succeeded));
    app.close().await;
}

#[tokio::test]
async fn partial_startup_failure_compensates_even_when_configuration_may_have_applied() {
    let mut app = Running::new(settings()).await;
    app.submit(1, LightingTarget::Processed).await;
    app.call(Call::Configure)
        .await
        .fail(ControlError::Unavailable("lost output configuration reply"));
    app.cleanup(0, false, IdleOutputMode::Off).await;
    assert!(matches!(app.result(1).await, OperationStatus::Failed(_)));
    app.state(|s| {
        matches!(
            s.session,
            SessionState::Failed {
                cleanup_complete: true,
                ..
            }
        )
    })
    .await;
    app.close().await;
}

#[tokio::test]
async fn failed_black_write_still_stops_capture_and_reports_failure() {
    let mut app = Running::new(settings()).await;
    app.start().await;
    app.submit(2, LightingTarget::Off).await;
    app.call(Call::Hold(epoch(0)))
        .await
        .fail(ControlError::Unavailable("black write failed"));
    app.call(Call::Stop(session())).await.ok();
    app.call(Call::Finish(session(), IdleOutputMode::Off))
        .await
        .ok();
    assert!(matches!(app.result(2).await, OperationStatus::Failed(_)));
    app.state(|s| {
        matches!(
            s.session,
            SessionState::Failed {
                cleanup_complete: true,
                ..
            }
        )
    })
    .await;
    app.close().await;
}

#[tokio::test]
async fn shutdown_awaits_delayed_finish_and_keeps_observation_responsive() {
    let mut app = Running::new(settings()).await;
    app.start().await;
    std::mem::replace(&mut app.shutdown, oneshot::channel().0)
        .send(())
        .unwrap();
    app.call(Call::Hold(epoch(0))).await.ok();
    app.call(Call::Stop(session())).await.ok();
    let finish = app.call(Call::Finish(session(), IdleOutputMode::Off)).await;
    assert!(!app.app.is_finished());
    app.state(|s| matches!(s.session, SessionState::Stopping(_)))
        .await;
    finish.ok();
    assert_eq!(within(app.app).await.unwrap().unwrap(), OwnerStopped);
    app.flow_shutdown.send(()).unwrap();
    within(app.flow).await.unwrap().unwrap();
}

#[tokio::test]
async fn bounded_operations_deduplicate_and_observers_get_explicit_resync() {
    let mut settings = settings();
    settings.output = None;
    let app = Running::new(settings).await;
    let mut observation = app.control.observe().await.unwrap();
    for request in 1..=70 {
        app.submit(request, LightingTarget::Off).await;
        assert!(matches!(
            app.result(request).await,
            OperationStatus::Succeeded
        ));
    }
    assert!(matches!(
        app.control.operation(id(1)).await,
        Err(ControlError::UnknownOperation)
    ));
    assert!(matches!(
        observation.updates.next().await.unwrap().unwrap(),
        Update::ResyncRequired
    ));
    assert!(observation.updates.next().await.is_none());
    app.submit(70, LightingTarget::Off).await;
    assert!(matches!(
        app.control
            .submit(Request {
                id: id(70),
                command: ApplicationCommand::SetTarget(LightingTarget::Processed)
            })
            .await,
        Err(ControlError::Conflict(_))
    ));
    let mut foreign = id(71);
    foreign.instance = InstanceId(NonZeroU64::new(2).unwrap());
    assert!(matches!(
        app.control
            .submit(Request {
                id: foreign,
                command: ApplicationCommand::SetTarget(LightingTarget::Off)
            })
            .await,
        Err(ControlError::Conflict(_))
    ));
    app.close().await;
}

#[tokio::test]
async fn capture_observation_gaps_reacquire_snapshot_and_admit_current_epoch() {
    let mut app = Running::new(settings()).await;
    app.start().await;
    {
        let mut snapshot = app.capture.snapshot.lock().unwrap();
        snapshot.state = CaptureState::Available(epoch(4));
        snapshot.cursor.sequence += 1;
    }
    app.capture.updates.send(Update::ResyncRequired).unwrap();
    app.call(Call::Admit(epoch(4))).await.ok();
    app.state(|s| matches!(s.session, SessionState::Running(_)))
        .await;
    app.submit(2, LightingTarget::Off).await;
    app.cleanup(4, true, IdleOutputMode::Off).await;
    app.result(2).await;
    app.close().await;
}

#[tokio::test]
async fn shutdown_supersedes_native_finish_without_dropping_the_inflight_write() {
    let mut app = Running::new(settings()).await;
    app.start().await;
    app.submit(2, LightingTarget::DeviceNative).await;
    app.call(Call::Hold(epoch(0))).await.ok();
    app.call(Call::Stop(session())).await.ok();
    let native = app
        .call(Call::Finish(session(), IdleOutputMode::Native))
        .await;
    std::mem::replace(&mut app.shutdown, oneshot::channel().0)
        .send(())
        .unwrap();
    app.state(|s| s.desired == LightingTarget::Off).await;
    assert!(!app.app.is_finished());
    native.ok();
    app.call(Call::Finish(session(), IdleOutputMode::Off))
        .await
        .ok();
    app.close().await;
}

#[tokio::test]
async fn explicit_off_supersedes_an_idle_native_transaction() {
    let mut app = Running::new(settings()).await;
    app.submit(1, LightingTarget::DeviceNative).await;
    app.call(Call::Configure).await.ok();
    let native = app
        .call(Call::Finish(session(), IdleOutputMode::Native))
        .await;
    app.submit(2, LightingTarget::Off).await;
    native.ok();
    app.call(Call::Finish(session(), IdleOutputMode::Off))
        .await
        .ok();
    assert!(matches!(app.result(1).await, OperationStatus::Failed(_)));
    assert!(matches!(app.result(2).await, OperationStatus::Succeeded));
    app.close().await;
}

#[tokio::test]
async fn shutdown_turns_off_an_already_idle_native_output() {
    let mut app = Running::new(settings()).await;
    app.submit(1, LightingTarget::DeviceNative).await;
    app.call(Call::Configure).await.ok();
    app.call(Call::Finish(session(), IdleOutputMode::Native))
        .await
        .ok();
    assert!(matches!(app.result(1).await, OperationStatus::Succeeded));
    std::mem::replace(&mut app.shutdown, oneshot::channel().0)
        .send(())
        .unwrap();
    let next = SessionId {
        sequence: NonZeroU64::new(2).unwrap(),
        ..session()
    };
    app.call(Call::Finish(next, IdleOutputMode::Off)).await.ok();
    app.close().await;
}

#[tokio::test]
async fn dropping_every_client_does_not_cancel_accepted_start_or_stop_the_owner() {
    let mut app = Running::new(settings()).await;
    let mut observation = app.control.observe().await.unwrap();
    app.submit(1, LightingTarget::Processed).await;
    let configure = app.call(Call::Configure).await;
    let (sender, _) = mpsc::channel(CAPACITY);
    drop(std::mem::replace(
        &mut app.control,
        ApplicationHandle(sender),
    ));
    configure.ok();
    app.call(Call::Hold(epoch(0))).await.ok();
    app.call(Call::Start(session())).await.ok();
    within(async {
        loop {
            if let Update::Snapshot(snapshot) = observation.updates.next().await.unwrap().unwrap()
                && matches!(snapshot.state.session, SessionState::Waiting(_))
            {
                break;
            }
        }
    })
    .await;
    assert!(!app.app.is_finished());
    assert!(app.calls.try_recv().is_err());
    std::mem::replace(&mut app.shutdown, oneshot::channel().0)
        .send(())
        .unwrap();
    app.cleanup(0, true, IdleOutputMode::Off).await;
    app.close().await;
}

#[tokio::test]
async fn final_output_mode_waits_for_the_real_processing_barrier() {
    let mut app = Running::new(settings()).await;
    app.start().await;
    app.capture.set(CaptureState::Available(epoch(1)));
    app.call(Call::Admit(epoch(1))).await.ok();
    let (release, gate) = std::sync::mpsc::channel();
    *app.color_gate.lock().unwrap() = Some(gate);
    app.frames
        .send(CapturedFrame {
            stamp: FrameStamp {
                epoch: epoch(1),
                sequence: 1,
            },
            frame: Frame::fill(150, 8, 4, PixelFormat::RGB24).unwrap(),
        })
        .await
        .unwrap();
    within(app.colors.recv()).await.unwrap();
    app.submit(2, LightingTarget::Off).await;
    app.call(Call::Hold(epoch(1))).await.ok();
    app.call(Call::Stop(session())).await.ok();
    app.state(|s| matches!(s.session, SessionState::Stopping(_)))
        .await;
    assert!(
        tokio::time::timeout(Duration::from_millis(20), app.calls.recv())
            .await
            .is_err()
    );
    release.send(()).unwrap();
    app.call(Call::Finish(session(), IdleOutputMode::Off))
        .await
        .ok();
    assert!(matches!(app.result(2).await, OperationStatus::Succeeded));
    app.close().await;
}

#[tokio::test]
async fn late_failure_observations_do_not_retry_blocked_cleanup() {
    let mut app = Running::new(settings()).await;
    app.start().await;
    app.submit(2, LightingTarget::Off).await;
    app.call(Call::Hold(epoch(0))).await.ok();
    app.call(Call::Stop(session()))
        .await
        .fail(ControlError::CleanupPending(session()));
    app.result(2).await;
    app.capture.set(CaptureState::Failed {
        session: session(),
        failure: prysm_contract::Failure {
            message: "stop timed out".into(),
        },
        cleanup_complete: false,
    });
    app.state(|s| matches!(s.observed_capture, Some(CaptureState::Failed { .. })))
        .await;
    assert!(app.calls.try_recv().is_err());
    app.submit(3, LightingTarget::Off).await;
    app.cleanup(0, true, IdleOutputMode::Off).await;
    assert!(matches!(app.result(3).await, OperationStatus::Succeeded));
    app.close().await;
}

#[tokio::test]
async fn observed_output_failure_cleans_up_without_claiming_device_confirmation() {
    let mut app = Running::new(settings()).await;
    app.start().await;
    let mut failed = app.output.snapshot.lock().unwrap().state.clone();
    failed.failure = Some(prysm_contract::Failure {
        message: "device disconnected".into(),
    });
    app.output.set(failed);
    app.cleanup(0, true, IdleOutputMode::Off).await;
    let state = app
        .state(|s| {
            matches!(
                s.session,
                SessionState::Failed {
                    cleanup_complete: true,
                    ..
                }
            )
        })
        .await;
    assert_eq!(state.desired, LightingTarget::Off);
    assert_eq!(
        state.observed_output.unwrap().confirmation,
        Some(OutputApplied::Sent)
    );
    app.close().await;
}

#[tokio::test]
async fn interrupted_processing_preparation_still_turns_off_prior_native_mode() {
    let mut app = Running::new(settings()).await;
    app.submit(1, LightingTarget::DeviceNative).await;
    app.call(Call::Configure).await.ok();
    app.call(Call::Finish(session(), IdleOutputMode::Native))
        .await
        .ok();
    app.result(1).await;
    // Pause the real worker to delay the next Start's processing configuration.
    app.processing
        .configure(prysm_contract::processing::ProcessingUpdate {
            revision: SettingsRevision(0),
            config: Default::default(),
        })
        .await
        .unwrap();
    let next = SessionId {
        sequence: NonZeroU64::new(2).unwrap(),
        ..session()
    };
    let next_epoch = StreamEpoch {
        session: next,
        generation: 1,
    };
    let (release, gate) = std::sync::mpsc::channel();
    *app.color_gate.lock().unwrap() = Some(gate);
    app.frames
        .send(CapturedFrame {
            stamp: FrameStamp {
                epoch: next_epoch,
                sequence: 1,
            },
            frame: Frame::fill(150, 8, 4, PixelFormat::RGB24).unwrap(),
        })
        .await
        .unwrap();
    within(app.colors.recv()).await.unwrap();
    app.submit(2, LightingTarget::Processed).await;
    app.submit(3, LightingTarget::Off).await;
    app.state(|s| matches!(s.session, SessionState::Stopping(_)))
        .await;
    release.send(()).unwrap();
    app.call(Call::Hold(StreamEpoch {
        session: next,
        generation: 0,
    }))
    .await
    .ok();
    app.call(Call::Finish(next, IdleOutputMode::Off)).await.ok();
    assert!(matches!(app.result(3).await, OperationStatus::Succeeded));
    app.close().await;
}

#[tokio::test]
async fn repository_needs_send_but_not_sync() {
    struct LocalRepository(std::cell::RefCell<TestSettings>);
    impl SettingsRepository<TestSettings> for LocalRepository {
        fn load(&self) -> impl Future<Output = Result<Option<TestSettings>, ControlError>> + Send {
            std::future::ready(Ok(Some(self.0.borrow().clone())))
        }
        fn save(&self, _: TestSettings) -> impl Future<Output = Result<(), ControlError>> + Send {
            std::future::ready(Err(ControlError::Unavailable("fixture is read only")))
        }
    }
    let app = Running::new(settings()).await;
    let (calls, _receiver) = mpsc::unbounded_channel();
    let application = Application::new(
        Capture {
            calls: Calls(calls.clone()),
            observed: app.capture.clone(),
        },
        app.processing.clone(),
        Output {
            calls: Calls(calls),
            observed: app.output.clone(),
        },
        LocalRepository(std::cell::RefCell::new(settings())),
        policy::V1Policy,
    );
    let (shutdown, stop) = oneshot::channel();
    let owner = tokio::spawn(application.owner.run(INSTANCE, async {
        let _ = stop.await;
    }));
    within(application.control.observe()).await.unwrap();
    shutdown.send(()).unwrap();
    within(owner).await.unwrap().unwrap();
    app.close().await;
}

#[tokio::test]
async fn stopping_before_the_first_start_poll_never_issues_a_late_open() {
    let app = Running::new(settings()).await;
    let (calls, mut receiver) = mpsc::unbounded_channel();
    let mut lifecycle = crate::session::Lifecycle::new(
        INSTANCE,
        (
            Capture {
                calls: Calls(calls.clone()),
                observed: app.capture.clone(),
            },
            app.processing.clone(),
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
                lifecycle.progress().await; // processing configuration
                lifecycle.progress().await; // output configuration
                lifecycle.progress().await; // initial black
            },
            async {
                let configure = receiver.recv().await.unwrap();
                assert_eq!(configure.call, Call::Configure);
                configure.ok();
                let black = receiver.recv().await.unwrap();
                assert_eq!(black.call, Call::Hold(epoch(0)));
                black.ok();
            }
        );
    })
    .await;
    // Opening is next, but has not been polled. Stop must never issue it later.
    lifecycle.request(LightingTarget::Off).unwrap();
    within(async {
        tokio::join!(
            async {
                while !lifecycle.is_idle() {
                    lifecycle.progress().await;
                }
            },
            async {
                let black = receiver.recv().await.unwrap();
                assert_eq!(black.call, Call::Hold(epoch(0)));
                black.ok();
                let finish = receiver.recv().await.unwrap();
                assert_eq!(finish.call, Call::Finish(session(), IdleOutputMode::Off));
                finish.ok();
            }
        );
    })
    .await;
    assert!(receiver.try_recv().is_err());
    app.close().await;
}
