//! prysm-webos: the full TV worker. webOS capture into WLED with the web
//! portal on the LAN, controlled by `prysm-webos-service` over its stdio.
mod link;
mod video;

use link::EngineLink;
use prysm_capture_webos::{VideoSignal, WebosCapture, WebosSettings};
use prysm_output_wled::Wled;
use prysm_ports::{AdapterSettings, Error, Result, Settings, SettingsStore, SourceFactory};
use prysm_runtime::Prysm;
use prysm_runtime::logging::{self, Logs};
use prysm_settings_file::FileSettings;
use prysm_web_portal::{Access, FrontendAsset, WebPortal};
use prysm_webos_link::FullArgs;
use prysm_webos_link::worker::{self, Link};
use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

const USAGE: &str =
    "prysm-webos --settings PATH --listen ADDR --lighting off|processed|device_native
Started by prysm-webos-service: commands on stdin, events on stdout, logs on stderr.";

const ASSETS: &[FrontendAsset] = include!("../../../frontend/dist/webos/web-portal/assets.rs");

/// Above the capture adapter's 4 s release limit, below the supervisor's 8 s
/// TERM-to-KILL window, so WLED idle compensation normally runs.
const STOP_DEADLINE: Duration = Duration::from_secs(6);

fn main() -> ExitCode {
    // stdout carries the supervisor protocol; log lines go to stderr only.
    logging::init("prysm-webos", Logs::Stderr);
    worker::main("prysm-webos", USAGE, FullArgs::parse, run)
}

async fn run(args: FullArgs, link: Link) -> Result<()> {
    seed(&args.settings)?;
    let listener = TcpListener::bind(args.listen)
        .map_err(|error| Error::adapter(format!("cannot listen on {}", args.listen), error))?;
    let address = listener
        .local_addr()
        .map_err(|error| Error::adapter("cannot read the portal address", error))?;
    let portal = portal_url(address);
    eprintln!(
        "prysm-webos: portal {}",
        portal.as_deref().unwrap_or("(no LAN address)")
    );
    let video = VideoSignal::default();
    let stopped = link.stop.stopped();
    Prysm::builder(FileSettings::new(&args.settings))
        .source(WebosCapture::new(video.clone()))
        .output(Wled::default())
        .surface(EngineLink::new(link, video, args.lighting, portal))
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
    let mut store = FileSettings::new(path);
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

/// The URL a phone on the LAN opens. For a wildcard listener this is the
/// address of the interface the default route uses (a connected UDP socket
/// sends nothing).
fn portal_url(address: SocketAddr) -> Option<String> {
    let ip = if address.ip().is_unspecified() {
        let probe = UdpSocket::bind("0.0.0.0:0").ok()?;
        probe.connect("192.0.2.1:9").ok()?;
        probe.local_addr().ok()?.ip()
    } else {
        address.ip()
    };
    (!ip.is_unspecified()).then(|| format!("http://{}/", SocketAddr::new(ip, address.port())))
}

#[cfg(test)]
mod tests;
