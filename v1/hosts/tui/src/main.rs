//! prysm-tui: the terminal previews what the LEDs show, next to the web portal.
use prysm_capture_device::DeviceSource;
use prysm_capture_remote::RemoteReceiver;
use prysm_output_wled::Wled;
use prysm_runtime::{NullOutput, Prysm};
use prysm_settings_file::FileSettings;
use prysm_surface_tui::Tui;
use prysm_web_portal::FrontendAsset;
use std::process::ExitCode;

const ABOUT: &str = "Preview capture in the terminal, optionally driving WLED; use output kind
\"null\" to test without LEDs. Quit with q, or :quit. Without an interactive
terminal the preview is unavailable and lighting and portal keep running.";

// The desktop hosts register the same adapters, so they share one portal bundle.
const ASSETS: &[FrontendAsset] = include!("../../../frontend/dist/headless/web-portal/assets.rs");

fn main() -> ExitCode {
    prysm_host::main("prysm-tui", ABOUT, |host| async move {
        let tui = Tui::new();
        let quit = tui.quit();
        let signals = host.signals()?;
        Prysm::builder(FileSettings::new(host.settings()))
            .source(DeviceSource)
            .source(RemoteReceiver)
            .output(Wled::default())
            .output(NullOutput)
            .surface(host.status().quiet_on_terminal())
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
