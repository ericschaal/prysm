use futures::{FutureExt, StreamExt};
use prysm_application::{
    Application,
    policy::{ApplicationPolicy, FailureAction, V1Policy},
};
use prysm_assemblies::{
    DeviceCaptureSettings, HostAction, OwnerExit, TvCaptureSettings, WledSettings, v1_host_action,
};
use prysm_contract::{
    ControlError,
    application::LightingTarget,
    capture::{CaptureCapabilities, CaptureControl, CaptureStart, CapturedFrame, FrameSource},
    identity::{FrameStamp, InstanceId, SessionId, SettingsRevision, StreamEpoch},
    observation::Update,
    preview::{PreviewLease, PreviewSource},
    processing::{ProcessingControl, ProcessingUpdate},
};
use prysm_core::{BrightnessPercent, Config, EdgeColors, Frame, PixelFormat};
use prysm_flow::{ProcessingExecution, ProcessingFlow};
use prysm_processor::PrysmProcessor;
use prysm_testkit::{Calls, FakeCapture, FakeOutput, MemorySettings};
use std::{
    num::NonZeroU64,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

async fn within<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("assembly made no progress")
}

fn session() -> SessionId {
    SessionId {
        instance: InstanceId(NonZeroU64::MIN),
        sequence: NonZeroU64::MIN,
    }
}

#[tokio::test]
async fn running_flow_configures_processes_and_reports_source_closure_without_a_portal() {
    let mut capture = FakeCapture::<DeviceCaptureSettings>::new(
        CaptureCapabilities {
            supports_patterns: false,
        },
        Calls::default(),
    );
    let output = FakeOutput::<WledSettings>::new(Calls::default());
    assert!(
        capture.frames.next_frame().now_or_never().is_none(),
        "idle is pending, not EOF"
    );
    let flow = ProcessingFlow::connect(
        capture.frames,
        PrysmProcessor::new(&Default::default()),
        output.colors,
        ProcessingExecution::DedicatedThread,
    );
    let owner = tokio::spawn(flow.owner.run(session().instance, std::future::pending()));
    let revision = SettingsRevision(7);
    let configured = within(flow.control.configure(ProcessingUpdate {
        revision,
        config: Config {
            brightness_percent: BrightnessPercent(0),
            ..Default::default()
        },
    }))
    .await
    .unwrap();
    assert_eq!(configured.revision, revision);
    let mut observation = within(flow.control.observe()).await.unwrap();
    assert_eq!(
        observation.initial.state.configured_revision,
        Some(revision)
    );
    assert_eq!(observation.initial.state.applied_revision, None);
    assert!(output.received.latest().is_none());
    let stamp = FrameStamp {
        epoch: StreamEpoch {
            session: session(),
            generation: 1,
        },
        sequence: 1,
    };
    capture
        .feed
        .send(CapturedFrame {
            stamp,
            frame: Frame::fill(255, 8, 4, PixelFormat::RGB24).unwrap(),
        })
        .await
        .unwrap();
    within(async {
        loop {
            match observation.updates.next().await.unwrap().unwrap() {
                Update::Snapshot(snapshot) if snapshot.state.applied_revision == Some(revision) => {
                    break;
                }
                Update::ResyncRequired => panic!("test observer lagged"),
                _ => {}
            }
        }
    })
    .await;
    let rendered = output.received.latest().unwrap();
    assert_eq!(rendered.stamp, stamp);
    assert_eq!(rendered.revision, revision);
    assert_eq!(rendered.colors, EdgeColors::black(8, 4, Default::default()));
    assert_eq!(
        within(flow.control.barrier(session()))
            .await
            .unwrap()
            .session,
        session()
    );
    drop(capture.feed);
    assert!(matches!(
        within(owner).await.unwrap(),
        Err(ControlError::Unavailable("capture frame owner closed"))
    ));
    assert!(within(flow.control.observe()).await.is_err());
    assert!(within(observation.updates.next()).await.is_none());
}

#[tokio::test]
async fn unavailable_observation_prevents_application_startup() {
    let capture = FakeCapture::<TvCaptureSettings>::new(
        CaptureCapabilities {
            supports_patterns: true,
        },
        Calls::default(),
    );
    let output = FakeOutput::<WledSettings>::new(Calls::default());
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
        MemorySettings::empty(),
        V1Policy,
    );
    assert!(matches!(
        application
            .owner
            .run(session().instance, std::future::pending())
            .await,
        Err(ControlError::Unavailable(_))
    ));
    assert!(matches!(
        flow.preview.subscribe().await,
        Err(ControlError::Unavailable(_))
    ));
}

#[tokio::test]
async fn preview_lease_owns_and_releases_its_demand_guard() {
    struct Guard(Arc<AtomicBool>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let released = Arc::new(AtomicBool::new(false));
    let stream = futures::stream::unfold(Guard(released.clone()), |guard| async {
        std::future::pending::<()>().await;
        Some((Err(ControlError::Unavailable("fixture")), guard))
    })
    .boxed();
    let lease = PreviewLease::new(stream);
    assert!(!released.load(Ordering::SeqCst));
    drop(lease);
    assert!(released.load(Ordering::SeqCst));
}

#[test]
fn v1_policies_are_separate_from_ports() {
    assert_eq!(V1Policy.initial_target(), LightingTarget::Off);
    assert_eq!(V1Policy.terminal_failure(), FailureAction::AwaitCommand);
    assert_eq!(v1_host_action(OwnerExit::WebPortal), HostAction::ReportOnly);
    assert_eq!(v1_host_action(OwnerExit::Preview), HostAction::ReportOnly);
    assert_eq!(
        v1_host_action(OwnerExit::Coordinator),
        HostAction::EmergencyCleanup
    );
    assert_eq!(
        v1_host_action(OwnerExit::Processing),
        HostAction::NotifyCoordinator
    );
}

#[tokio::test]
async fn remote_receiver_exposes_capture_ports_without_claiming_transport_success() {
    use prysm_remote_capture::{RemoteCaptureReceiver, RemoteCaptureSettings};
    let receiver = RemoteCaptureReceiver::new(CaptureCapabilities {
        supports_patterns: true,
    });
    let settings = RemoteCaptureSettings {
        peer: "127.0.0.1:9000".parse().unwrap(),
    };
    assert!(receiver.control.capabilities().supports_patterns);
    assert!(matches!(
        receiver.control.validate(&settings),
        Err(ControlError::Unavailable(_))
    ));
    assert!(matches!(
        receiver
            .control
            .start(CaptureStart {
                session: session(),
                settings
            })
            .await,
        Err(ControlError::Unavailable(_))
    ));
    assert!(matches!(
        receiver.control.stop(session()).await,
        Err(ControlError::Unavailable(_))
    ));
    assert!(matches!(
        receiver.control.observe().await,
        Err(ControlError::Unavailable(_))
    ));
    let output = FakeOutput::<WledSettings>::new(Calls::default());
    let flow = ProcessingFlow::connect(
        receiver.frames,
        PrysmProcessor::new(&Default::default()),
        output.colors,
        ProcessingExecution::DedicatedThread,
    );
    assert!(matches!(
        within(flow.owner.run(session().instance, std::future::pending())).await,
        Err(ControlError::Unavailable(_))
    ));
    assert!(output.received.latest().is_none());
    assert!(matches!(
        receiver.owner.run(std::future::pending()).await,
        Err(ControlError::Unavailable(_))
    ));
}
