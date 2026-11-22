//! prysm-webos: the full TV worker. webOS capture into WLED with the web
//! portal on the LAN, controlled by `prysm-webos-service` over its stdio.
mod link;

use link::EngineLink;
use prysm_capture_webos::{VideoSignal, WebosCapture, WebosSettings};
use prysm_host::assets;
use prysm_output_wled::Wled;
use prysm_ports::{AdapterSettings, Error, Result, Settings, SettingsStore, SourceFactory};
use prysm_runtime::Prysm;
use prysm_runtime::logging::{self, Logs};
use prysm_settings_file::SettingsFile;
use prysm_web_portal::{Access, WebPortal, lan_url};
use prysm_webos_link::worker::{self, Link};
use prysm_webos_link::{FullArgs, KILL_AFTER};
use std::net::TcpListener;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

const NAME: &str = "prysm-webos";

const ABOUT: &str = "The full TV worker. Started by prysm-webos-service: commands on stdin,
events on stdout, logs on stderr.";

/// Above the capture adapter's release limit plus the output's idle limit,
/// so a stop that meets nothing in flight idles the lights in time. A stop
/// that meets an open or a release in flight can exceed it (see
/// [`prysm_output_wled::STOP_WAIT`]); the engine then faults.
const STOP_DEADLINE: Duration = Duration::from_secs(6);
/// How long shutdown waits for an engine call stuck in an adapter before
/// abandoning it: short, so grace and stop (1 + 6 s) end before SIGKILL (8 s).
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);
const _: () = assert!(
    prysm_capture_webos::RELEASE
        .saturating_add(prysm_output_wled::IDLE_LIMIT)
        .as_millis()
        < STOP_DEADLINE.as_millis(),
    "a stop that meets nothing in flight must end before the stop deadline"
);
const _: () = assert!(
    SHUTDOWN_GRACE.as_millis() + STOP_DEADLINE.as_millis() < KILL_AFTER.as_millis(),
    "shutdown must end before the supervisor's SIGKILL"
);

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
        // The TV app is a `file://` page (`Origin: null`); it needs CORS. A
        // phone opens the portal's own pages, which are always served.
        .surface(WebPortal::from_listener(
            listener,
            assets::WEBOS,
            Access::Origins(vec!["null".into()]),
        )?)
        .stop_deadline(STOP_DEADLINE)
        .shutdown_grace(SHUTDOWN_GRACE)
        .run(stopped)
        .await
}

/// A missing settings file starts as webOS capture with defaults, so the
/// portal only has to add the WLED output.
fn seed(path: &Path) -> Result<()> {
    let mut store = SettingsFile::new(path);
    // Only a missing file: one that cannot be read is the engine's to report.
    if matches!(store.load(), Ok(None)) {
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
