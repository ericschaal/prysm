//! prysm-headless: local or remote capture into WLED, driven by the web portal.
use prysm_capture_device::DeviceSource;
use prysm_host::{Program, assets};
use std::process::ExitCode;

const ABOUT: &str = "Capture from a local video device (kind \"device\"), a capture-only TV
(kind \"remote\") or a video file played once through FFmpeg 9+ on PATH (kind
\"file\") into WLED (kind \"wled\"), controlled from the web portal. Output
kind \"null\" runs the pipeline without LEDs.";

const PROGRAM: Program = Program {
    name: "prysm-headless",
    about: ABOUT,
};

// A stop with the device source ends before the stop deadline.
const _: () = assert!(prysm_host::stops_in_time(
    prysm_capture_device::RELEASE_LIMIT
));

fn main() -> ExitCode {
    // A capture-only TV streams to the portal, which hands it to the remote source.
    let (adapters, tv_stream) = prysm_host::desktop_adapters(DeviceSource);
    prysm_host::main(PROGRAM, adapters, |host, prysm| async move {
        prysm
            .surface(host.status())
            .surface(host.start())
            .surface(host.portal(assets::HEADLESS)?.capture_stream(tv_stream))
            .run(host.signals())
            .await
    })
}
