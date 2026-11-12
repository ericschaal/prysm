//! prysm-headless: local or remote capture into WLED, driven by the web portal.
use prysm_capture_device::DeviceSource;
use prysm_capture_file::FileSource;
use prysm_capture_remote::RemoteReceiver;
use prysm_host::Program;
use prysm_output_wled::Wled;
use prysm_runtime::{NullOutput, Prysm};
use prysm_settings_file::FileSettings;
use prysm_web_portal::FrontendAsset;
use std::process::ExitCode;

const ABOUT: &str = "Capture from a local video device (kind \"device\"), a capture-only TV
(kind \"remote\") or a video file played once through FFmpeg 9+ on PATH (kind
\"file\") into WLED (kind \"wled\"), controlled from the web portal. Output
kind \"null\" runs the pipeline without LEDs.";

const ASSETS: &[FrontendAsset] = include!("../../../frontend/dist/headless/web-portal/assets.rs");

const PROGRAM: Program = Program {
    name: "prysm-headless",
    about: ABOUT,
    full_screen: false,
};

fn main() -> ExitCode {
    prysm_host::main(PROGRAM, |host| async move {
        Prysm::builder(FileSettings::new(host.settings()))
            .source(DeviceSource)
            .source(FileSource)
            .source(RemoteReceiver)
            .output(Wled::default())
            .output(NullOutput)
            .surface(host.status())
            .surface(host.portal(ASSETS)?)
            .run(host.signals()?)
            .await
    })
}
