//! prysm-webos-slim: the slim TV worker. webOS capture forwarded to a
//! headless Prysm's frame receiver, controlled by `prysm-webos-service`.
mod link;
#[path = "../../webos/src/video.rs"]
mod video;

use link::{ForwarderLink, Uplink};
use prysm_capture_remote::RemoteSender;
use prysm_capture_webos::{VideoSignal, WebosCapture, WebosSettings};
use prysm_ports::Result;
use prysm_runtime::Forwarder;
use prysm_runtime::logging::{self, Logs};
use prysm_webos_link::worker::{self, Link};
use prysm_webos_link::{SlimArgs, SlimCapture};
use std::process::ExitCode;
use std::time::Duration;

const USAGE: &str =
    "prysm-webos-slim --receiver ADDR --width N --height N --fps N --include-ui true|false
Started by prysm-webos-slim-service: commands on stdin, events on stdout, logs on stderr.";

/// Above the capture adapter's 4 s release limit, below the supervisor's 8 s
/// TERM-to-KILL window.
const STOP_DEADLINE: Duration = Duration::from_secs(6);

fn main() -> ExitCode {
    // stdout carries the supervisor protocol; log lines go to stderr only.
    logging::init("prysm-webos-slim", Logs::Stderr);
    worker::main("prysm-webos-slim", USAGE, SlimArgs::parse, run)
}

/// The receiver listens only while its host is lighting, so the uplink
/// reconnects on its own and an unavailable receiver is reported in the
/// status rather than ending the worker (which would back off and show a
/// failure). Capture runs meanwhile; frames are dropped until it connects.
async fn run(args: SlimArgs, link: Link) -> Result<()> {
    let receiver = args.receiver;
    let (sink, flow) = Uplink::new(move || RemoteSender::connect(receiver));
    let video = VideoSignal::default();
    let source = WebosCapture::new(video.clone()).start(&settings(args.capture))?;
    let stopped = link.stop.stopped();
    Forwarder::new(source, sink)
        .surface(ForwarderLink::new(link, video, flow))
        .stop_deadline(STOP_DEADLINE)
        .run(stopped)
        .await
}

fn settings(capture: SlimCapture) -> WebosSettings {
    WebosSettings {
        width: capture.width,
        height: capture.height,
        fps: capture.fps,
        include_ui: capture.include_ui,
    }
}

#[cfg(test)]
mod tests;
