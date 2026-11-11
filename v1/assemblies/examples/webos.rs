//! Native webOS composition compiled on the development host using explicit fixtures.
use prysm_application::{Application, policy::V1Policy};
use prysm_assemblies::tv::{LunaApplicationBridge, LunaPlatformBridge};
use prysm_assemblies::{TvCaptureSettings, WledSettings};
use prysm_contract::capture::CaptureCapabilities;
use prysm_flow::{ProcessingExecution, ProcessingFlow};
use prysm_processor::PrysmProcessor;
use prysm_testkit::{Calls, FakeCapture, FakeOutput, MemorySettings};
use prysm_web_portal::{FrontendAsset, HttpPolicy, WebPortal};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Always package the TV UI. This host also includes the browser portal below.
    let tv_ui = include_str!("../../frontend/dist/webos/tv-ui/index.html");
    assert!(tv_ui.contains("TV composition fixture"));
    // Future WebosCapture receives a Luna platform gate; it needs no shared app flag.
    let capture = FakeCapture::<TvCaptureSettings>::new(
        CaptureCapabilities {
            supports_patterns: true,
        },
        Calls::default(),
    );
    let output = FakeOutput::<WledSettings>::new(Calls::default());
    // Built by `npm --prefix v1/frontend run build`; embeds every file in this host's bundle.
    let frontend_assets: &'static [FrontendAsset] =
        include!("../../frontend/dist/webos/web-portal/assets.rs");
    let flow = ProcessingFlow::connect(
        capture.frames,
        PrysmProcessor::new(&prysm_core::Config::default()),
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
    let _portal = WebPortal::new(
        application.control.clone(),
        Some(flow.preview),
        frontend_assets,
        HttpPolicy {
            allowed_authorities: vec!["prysm-tv.local:8787".into()],
        },
    );
    let tv_application = LunaApplicationBridge::new(application.control.clone());
    let tv_platform = LunaPlatformBridge;

    // The future TV host also owns the platform bridge and outer process supervision.
    let _owners = (capture.owner, flow.owner, output.owner, application.owner);
    let _frame_producer = capture.feed;
    let _tv_bridges = (tv_application, tv_platform);
    println!("webOS wiring constructed; no SDK, native capture or portal was started.");
}
