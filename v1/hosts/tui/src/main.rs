//! prysm-tui: the terminal previews what the LEDs show, next to the web portal.
use prysm_capture_device::DeviceSource;
use prysm_capture_file::FileSource;
use prysm_capture_remote::RemoteReceiver;
use prysm_host::Program;
use prysm_output_wled::Wled;
use prysm_runtime::{Builder, NullOutput};
use prysm_surface_tui::Tui;
use prysm_web_portal::FrontendAsset;
use std::process::ExitCode;

const ABOUT: &str = "Preview capture in the terminal, optionally driving WLED; use output kind
\"null\" to test without LEDs. Capture kinds: \"device\", \"remote\" and \"file\" (a
video played once through FFmpeg 9+ on PATH; see settings.file.example.json).
Quit with q, or :quit. Without an interactive
terminal the preview is unavailable and lighting and portal keep running.
While the preview uses the terminal, status is not printed and log lines go
only to --log-file.";

// The desktop hosts register the same adapters, so they share one portal bundle.
const ASSETS: &[FrontendAsset] = include!("../../../frontend/dist/headless/web-portal/assets.rs");

const PROGRAM: Program = Program {
    name: "prysm-tui",
    about: ABOUT,
    full_screen: true,
};

fn main() -> ExitCode {
    prysm_host::main(PROGRAM, adapters, |host, prysm| async move {
        let tui = Tui::new();
        let quit = tui.quit();
        let signals = host.signals()?;
        prysm
            .surface(host.status())
            .surface(host.start())
            .surface(host.portal(ASSETS)?)
            .surface(tui)
            .run(async {
                tokio::select! {
                    () = signals => {}
                    () = quit.requested() => {}
                }
            })
            .await
    })
}

/// The capture sources and outputs this host can use.
fn adapters(prysm: Builder) -> Builder {
    prysm
        .source(DeviceSource)
        .source(FileSource)
        .source(RemoteReceiver)
        .output(Wled::default())
        .output(NullOutput)
}
