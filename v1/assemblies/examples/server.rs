//! Embed the built frontend and construct wiring; no hardware or HTTP listener starts.
use prysm_application::{Application, policy::V1Policy};
use prysm_assemblies::{EmergencyControls, ServerCaptureSettings, WledSettings};
use prysm_contract::capture::CaptureCapabilities;
use prysm_flow::{ProcessingExecution, ProcessingFlow};
use prysm_processor::PrysmProcessor;
use prysm_testkit::{Calls, FakeCapture, FakeOutput, MemorySettings};
use prysm_web_portal::{FrontendAsset, HttpPolicy, WebPortal};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // This fixture stands in for host-owned selection between video device capture and
    // RemoteCaptureReceiver. Runtime switching/ownership is deferred.
    let capture = FakeCapture::<ServerCaptureSettings>::new(
        CaptureCapabilities {
            supports_patterns: true,
        },
        Calls::default(),
    );
    let output = FakeOutput::<WledSettings>::new(Calls::default());
    // Built by `npm --prefix v1/frontend run build`; embeds every file in this host's bundle.
    let frontend_assets: &'static [FrontendAsset] =
        include!("../../frontend/dist/server/web-portal/assets.rs");
    let flow = ProcessingFlow::connect(
        capture.frames,
        PrysmProcessor::new(&prysm_core::Config::default()),
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
    let _portal = WebPortal::new(
        application.control.clone(),
        Some(flow.preview),
        frontend_assets,
        HttpPolicy {
            allowed_authorities: vec!["localhost:8787".into()],
        },
    );

    // The future host supervisor owns these parts, including independent receiver I/O.
    let _owners = (capture.owner, flow.owner, output.owner, application.owner);
    let _emergency = emergency;
    let _frame_producer = capture.feed;
    println!("prysm-server wiring constructed; production behavior is intentionally unavailable.");
}
