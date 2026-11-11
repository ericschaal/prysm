//! Capture-only TV: always package the TV UI; never assemble a local processor or portal.
use prysm_assemblies::TvCaptureSettings;
use prysm_assemblies::tv::LunaPlatformBridge;
use prysm_contract::{ControlError, capture::CaptureCapabilities};
use prysm_remote_capture::RemoteCaptureSender;
use prysm_testkit::{Calls, FakeCapture};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Packaging takes the whole dist/webos-slim/tv-ui directory, including its scripts.
    let tv_ui = include_str!("../../frontend/dist/webos-slim/tv-ui/index.html");
    assert!(tv_ui.contains("TV composition fixture"));
    let _tv_platform = LunaPlatformBridge;
    // The TV UI receives its remote ApplicationClient separately; there is no local coordinator.
    let capture = FakeCapture::<TvCaptureSettings>::new(
        CaptureCapabilities {
            supports_patterns: true,
        },
        Calls::default(),
    );
    let sender = RemoteCaptureSender::connect(
        capture.control,
        capture.frames,
        "192.0.2.10:9000".parse().unwrap(),
    );
    let _owners = (capture.owner, capture.feed);
    assert!(matches!(
        sender.run(std::future::pending()).await,
        Err(ControlError::Unavailable(_))
    ));
    println!("Capture-only TV wiring constructed with packaged UI; transport is unavailable.");
}
