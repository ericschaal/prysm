//! prysm-headless: local or remote capture into WLED, driven by the web portal.
use prysm_capture_device::DeviceSource;
use prysm_host::Program;
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
    prysm_capture_device::STOP,
    prysm_host::STOP_DEADLINE
));

fn main() -> ExitCode {
    prysm_host::main(PROGRAM, DeviceSource, |host, prysm| async move {
        prysm.run(host.signals()).await
    })
}
