//! prysm-webos: the full TV worker. webOS capture into WLED with the web
//! portal on the LAN, controlled by `prysm-webos-service` over its stdio.
mod link;

use link::EngineLink;
use prysm_capture_webos::{VideoSignal, WebosCapture, WebosSettings};
use prysm_output_wled::Wled;
use prysm_ports::{AdapterSettings, Error, Result, Settings, SettingsStore, SourceFactory};
use prysm_runtime::Prysm;
use prysm_runtime::logging::{self, Logs};
use prysm_settings_file::SettingsFile;
use prysm_web_portal::{Access, FrontendAsset, WebPortal, lan_url};
use prysm_webos_link::FullArgs;
use prysm_webos_link::worker::{self, Link};
use std::net::TcpListener;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

const NAME: &str = "prysm-webos";

const ABOUT: &str = "The full TV worker. Started by prysm-webos-service: commands on stdin,
events on stdout, logs on stderr.";

const ASSETS: &[FrontendAsset] = include!("../../../frontend/dist/webos/web-portal/assets.rs");

/// Above the capture adapter's 4 s release limit, below the supervisor's 8 s
/// TERM-to-KILL window, so WLED idle compensation normally runs.
const STOP_DEADLINE: Duration = Duration::from_secs(6);

fn main() -> ExitCode {
    // stdout carries the supervisor protocol; log lines go to stderr only.
    logging::init(NAME, Logs::Stderr);
    worker::main(NAME, worker::args::<FullArgs>(NAME, ABOUT), run)
}

async fn run(args: FullArgs, link: Link) -> Result<()> {
    seed(&args.settings)?;
    let listener = TcpListener::bind(args.listen)
        .map_err(|error| Error::adapter(format!("cannot listen on {}", args.listen), error))?;
    let address = listener
        .local_addr()
        .map_err(|error| Error::adapter("cannot read the portal address", error))?;
    // The URL a phone on the LAN opens; none rather than a loopback address.
    // Looked up again at every heartbeat: the TV's address can change.
    eprintln!(
        "prysm-webos: portal {}",
        lan_url(address).as_deref().unwrap_or("(no LAN address)")
    );
    let video = VideoSignal::default();
    let stopped = link.stop.stopped();
    Prysm::builder(SettingsFile::new(&args.settings))
        .source(WebosCapture::new(video.clone()))
        .output(Wled::default())
        .surface(EngineLink::new(
            link,
            video,
            args.lighting,
            Box::new(move || lan_url(address)),
        ))
        // The TV app is a `file://` page (`Origin: null`); it needs CORS.
        .surface(WebPortal::from_listener(
            listener,
            ASSETS,
            Access::AnyOrigin,
        )?)
        .stop_deadline(STOP_DEADLINE)
        .run(stopped)
        .await
}

/// A missing settings file starts as webOS capture with defaults, so the
/// portal only has to add the WLED output.
fn seed(path: &Path) -> Result<()> {
    let mut store = SettingsFile::new(path);
    if store.load()?.is_none() {
        store.save(&Settings {
            capture: Some(AdapterSettings::new(
                WebosCapture::KIND,
                &WebosSettings::default(),
            )),
            ..Settings::default()
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
