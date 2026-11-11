use super::*;
use prysm_application::{Application, policy::ApplicationPolicy};
use prysm_contract::{
    application::{
        ApplicationCommand, ApplicationControl, LightingTarget, OperationStatus, Request,
        SessionState, Settings,
    },
    identity::OperationId,
    observation::{Cursor, Versioned},
    output::{
        ColorFrame, ColorSink, IdleOutputMode, OfferResult, OutputApplied, OutputCapabilities,
        OutputControl, OutputState,
    },
    storage::SettingsRepository,
};
use prysm_core::{EdgeColors, SmoothingSeconds};
use prysm_flow::{ProcessingExecution, ProcessingFlow};
use prysm_processor::PrysmProcessor;

#[derive(Clone)]
struct Output {
    state: watch::Sender<Versioned<OutputState>>,
    colors: mpsc::UnboundedSender<ColorFrame>,
    finished: mpsc::UnboundedSender<SessionId>,
}
impl Output {
    fn set(&self, admitted: Option<StreamEpoch>, held: bool, idle_mode: Option<IdleOutputMode>) {
        self.state.send_modify(|snapshot| {
            snapshot.cursor.sequence += 1;
            snapshot.state = OutputState {
                admitted,
                held,
                idle_mode,
                confirmation: Some(OutputApplied::Sent),
                failure: None,
            };
        });
    }
}
impl ColorSink for Output {
    fn offer(&self, frame: ColorFrame) -> Result<OfferResult, ControlError> {
        self.colors.send(frame).unwrap();
        Ok(OfferResult::Queued)
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
        Ok(OutputApplied::Sent)
    }
    async fn hold(&self, _: StreamEpoch, _: EdgeColors) -> Result<OutputApplied, ControlError> {
        self.set(None, true, None);
        Ok(OutputApplied::Sent)
    }
    async fn admit(&self, epoch: StreamEpoch) -> Result<OutputApplied, ControlError> {
        self.set(Some(epoch), false, None);
        Ok(OutputApplied::Sent)
    }
    async fn finish(
        &self,
        session: SessionId,
        mode: IdleOutputMode,
    ) -> Result<OutputApplied, ControlError> {
        self.set(None, false, Some(mode));
        self.finished.send(session).unwrap();
        Ok(OutputApplied::Sent)
    }
    async fn observe(&self) -> Result<Observation<OutputState>, ControlError> {
        let mut updates = self.state.subscribe();
        let initial = updates.borrow_and_update().clone();
        Ok(Observation {
            initial,
            updates: futures::stream::unfold(updates, |mut updates| async {
                updates.changed().await.ok()?;
                let snapshot = updates.borrow_and_update().clone();
                Some((Ok(Update::Snapshot(snapshot)), updates))
            })
            .boxed(),
        })
    }
}
type AppSettings = Settings<DeviceCaptureSettings, ()>;
struct Repository(AppSettings);
impl SettingsRepository<AppSettings> for Repository {
    async fn load(&self) -> Result<Option<AppSettings>, ControlError> {
        Ok(Some(self.0.clone()))
    }
    async fn save(&self, _: AppSettings) -> Result<(), ControlError> {
        panic!("fixture cannot save")
    }
}
fn operation(sequence: u64) -> OperationId {
    OperationId {
        instance: INSTANCE,
        request: NonZeroU64::new(sequence).unwrap(),
    }
}

#[tokio::test]
async fn application_and_flow_wait_for_native_readiness_and_release() {
    let capture = DeviceCapture::new(INSTANCE);
    let (factory, mut opened) = factory();
    let (capture_shutdown, capture_stop) = oneshot::channel();
    let capture_owner = tokio::spawn(owner::run(
        capture.owner,
        async {
            let _ = capture_stop.await;
        },
        factory,
    ));
    let (colors, mut frames) = mpsc::unbounded_channel();
    let (finished, mut finishes) = mpsc::unbounded_channel();
    let output = Output {
        colors,
        finished,
        state: watch::channel(Versioned {
            cursor: Cursor {
                instance: INSTANCE,
                sequence: 0,
            },
            state: OutputState {
                admitted: None,
                held: false,
                idle_mode: None,
                confirmation: None,
                failure: None,
            },
        })
        .0,
    };
    let processing = prysm_core::Config {
        smoothing_seconds: SmoothingSeconds(0.0),
        remove_black_bars: false,
        ..Default::default()
    };
    let flow = ProcessingFlow::connect(
        capture.frames,
        PrysmProcessor::new(&processing),
        output.clone(),
        ProcessingExecution::DedicatedThread,
    );
    let (flow_shutdown, flow_stop) = oneshot::channel();
    let flow_owner = tokio::spawn(flow.owner.run(INSTANCE, async {
        let _ = flow_stop.await;
    }));
    let app = Application::new(
        capture.control.clone(),
        flow.control,
        output,
        Repository(Settings {
            capture: Some(settings()),
            output: Some(()),
            processing,
            profiles: vec![],
            active_profile: None,
        }),
        ApplicationPolicy::default(),
    );
    let (app_shutdown, app_stop) = oneshot::channel();
    let app_owner = tokio::spawn(app.owner.run(INSTANCE, async {
        let _ = app_stop.await;
    }));
    app.control
        .submit(Request {
            id: operation(1),
            command: ApplicationCommand::SetTarget(LightingTarget::Processed),
        })
        .await
        .unwrap();
    let mut native = within(opened.recv()).await.unwrap();
    assert!(matches!(
        app.control.operation(operation(1)).await.unwrap(),
        OperationStatus::Pending
    ));
    native.ready();
    let mut observation = app.control.observe().await.unwrap();
    within(async {
        loop {
            if matches!(observation.initial.state.session, SessionState::Waiting(_)) {
                break;
            }
            if let Update::Snapshot(snapshot) = observation.updates.next().await.unwrap().unwrap() {
                observation.initial = snapshot;
            }
        }
    })
    .await;
    native.frames.send(Some(frame(255))).unwrap();
    let colors = within(frames.recv()).await.unwrap();
    assert_eq!(
        colors.stamp.epoch,
        StreamEpoch {
            session: session(1),
            generation: 1
        }
    );
    assert!(matches!(
        app.control.operation(operation(1)).await.unwrap(),
        OperationStatus::Succeeded
    ));
    app.control
        .submit(Request {
            id: operation(2),
            command: ApplicationCommand::SetTarget(LightingTarget::Off),
        })
        .await
        .unwrap();
    within(native.stop.cancelled()).await;
    assert!(matches!(
        app.control.operation(operation(2)).await.unwrap(),
        OperationStatus::Pending
    ));
    assert!(finishes.try_recv().is_err());
    native.finish();
    assert_eq!(within(finishes.recv()).await.unwrap(), session(1));
    app_shutdown.send(()).unwrap();
    within(app_owner).await.unwrap().unwrap();
    flow_shutdown.send(()).unwrap();
    within(flow_owner).await.unwrap().unwrap();
    capture_shutdown.send(()).unwrap();
    within(capture_owner).await.unwrap().unwrap();
}
