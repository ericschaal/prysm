use futures::{FutureExt, StreamExt};
use prysm_application::{
    Application,
    policy::{ApplicationPolicy, FailureAction, V1Policy},
};
use prysm_assemblies::{
    DesktopCaptureSettings, EmergencyControls, HostAction, OwnerExit, TvCaptureSettings,
    WledSettings, v1_host_action,
};
use prysm_contract::{
    ControlError,
    application::LightingTarget,
    capture::{CaptureCapabilities, CaptureControl, CaptureStart, CapturedFrame, FrameSource},
    identity::{FrameStamp, InstanceId, SessionId, SettingsRevision, StreamEpoch},
    output::{IdleOutputMode, OutputControl},
    preview::{PreviewLease, PreviewSource},
};
use prysm_core::Frame;
use prysm_flow::{ProcessingExecution, ProcessingFlow};
use prysm_processor::PrysmProcessor;
use prysm_testkit::{Call, Calls, FakeCapture, FakeOutput, MemorySettings, process_one};
use std::{
    num::NonZeroU64,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

fn session() -> SessionId {
    SessionId {
        instance: InstanceId(NonZeroU64::MIN),
        sequence: NonZeroU64::MIN,
    }
}

#[tokio::test]
async fn real_processor_connects_fake_source_to_output_without_a_portal() {
    let capture = FakeCapture::<DesktopCaptureSettings>::new(
        CaptureCapabilities {
            supports_patterns: false,
        },
        Calls::default(),
    );
    let output = FakeOutput::<WledSettings>::new(Calls::default());
    let mut flow = ProcessingFlow::connect(
        capture.frames,
        PrysmProcessor::new(&Default::default()),
        output.colors,
        ProcessingExecution::DedicatedThread,
    );
    assert!(
        flow.owner
            .wiring
            .frames
            .next_frame()
            .now_or_never()
            .is_none(),
        "idle is pending, not EOF"
    );
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
            frame: Frame::dummy(4, 2).unwrap(),
        })
        .await
        .unwrap();
    let revision = SettingsRevision(7);
    assert!(process_one(&mut flow.owner.wiring, revision).await.unwrap());
    let rendered = output.received.latest().unwrap();
    assert_eq!(rendered.stamp, stamp);
    assert_eq!(rendered.revision, revision);
    drop(capture.feed);
    assert!(
        !process_one(&mut flow.owner.wiring, revision).await.unwrap(),
        "closure is EOF"
    );
}

#[tokio::test]
async fn host_keeps_emergency_controls_after_coordinator_is_dropped() {
    let calls = Calls::default();
    let capture = FakeCapture::<TvCaptureSettings>::new(
        CaptureCapabilities {
            supports_patterns: true,
        },
        calls.clone(),
    );
    let output = FakeOutput::<WledSettings>::new(calls.clone());
    let flow = ProcessingFlow::connect(
        capture.frames,
        PrysmProcessor::new(&Default::default()),
        output.colors,
        ProcessingExecution::DedicatedThread,
    );
    let emergency = EmergencyControls {
        capture: capture.control.clone(),
        processing: flow.control.clone(),
        output: output.control.clone(),
    };
    let application = Application::new(
        capture.control,
        flow.control,
        output.control,
        MemorySettings::empty(),
        V1Policy,
    );
    let settings = TvCaptureSettings {
        resolution: Default::default(),
        rate: 30.try_into().unwrap(),
    };
    application
        .owner
        .wiring
        .capture
        .start(CaptureStart {
            session: session(),
            settings,
        })
        .await
        .unwrap();
    drop(application);
    emergency.capture.stop(session()).await.unwrap();
    emergency
        .output
        .finish(session(), IdleOutputMode::Off)
        .await
        .unwrap();
    assert_eq!(
        calls.recorded(),
        [
            Call::CaptureStart(session()),
            Call::CaptureStop(session()),
            Call::OutputFinish(session(), IdleOutputMode::Off)
        ]
    );
    // This proves connection ownership, not the unimplemented cleanup/fencing protocol.
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
    let mut flow = ProcessingFlow::connect(
        receiver.frames,
        PrysmProcessor::new(&Default::default()),
        output.colors,
        ProcessingExecution::DedicatedThread,
    );
    assert!(matches!(
        flow.owner.wiring.frames.next_frame().await,
        Err(ControlError::Unavailable(_))
    ));
    assert!(output.received.latest().is_none());
    assert!(matches!(
        receiver.owner.run(std::future::pending()).await,
        Err(ControlError::Unavailable(_))
    ));
}
