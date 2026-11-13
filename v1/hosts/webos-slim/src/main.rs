//! prysm-webos-slim: the slim TV worker. webOS capture forwarded to a
//! headless Prysm's frame receiver, controlled by `prysm-webos-service`.
mod capture;
mod check;
mod link;
mod resolve;

use capture::RequestedCapture;
use clap::{Parser, Subcommand};
use link::{ForwarderLink, Uplink};
use prysm_capture_remote::RemoteSender;
use prysm_capture_webos::{VideoSignal, WebosCapture};
use prysm_ports::{FrameSource, Result};
use prysm_runtime::Forwarder;
use prysm_runtime::logging::{self, Logs};
use prysm_webos_link::SlimArgs;
use prysm_webos_link::worker::{self, Link};
use resolve::Resolver;
use std::process::ExitCode;
use std::time::Duration;

const NAME: &str = "prysm-webos-slim";

const ABOUT: &str = "The slim TV worker. Started by prysm-webos-slim-service: commands on stdin,
events on stdout, logs on stderr. The receiver's request sets what is captured
and how it is encoded.";

/// The worker's arguments, or the encoder check.
#[derive(Debug, Parser)]
#[command(
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true,
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    check: Option<Check>,
    /// `None` only with a subcommand.
    #[command(flatten)]
    worker: Option<SlimArgs>,
}

#[derive(Debug, Subcommand)]
enum Check {
    /// A bounded encoder check for the TV; not used by the supervisor
    CheckEncoder(check::Options),
}

/// Above the capture adapter's 4 s release limit, below the supervisor's 8 s
/// TERM-to-KILL window.
const STOP_DEADLINE: Duration = Duration::from_secs(6);

fn main() -> ExitCode {
    // stdout carries the supervisor protocol; log lines go to stderr only.
    logging::init(NAME, Logs::Stderr);
    let cli: Cli = worker::args(NAME, ABOUT);
    match cli.check {
        Some(Check::CheckEncoder(options)) => check::main(options),
        // Without a subcommand clap requires --receiver.
        None => worker::main(NAME, cli.worker.expect("--receiver is required"), run),
    }
}

/// The receiver listens only while its host is lighting, so the uplink
/// reconnects on its own and an unavailable receiver is reported in the
/// status rather than ending the worker (which would back off and show a
/// failure). Each connection brings the receiver's request: the capture
/// follows its size, rate and menus, the sender its encoding. A failing
/// encoder is reported and refused to the receiver until the request
/// changes; a failing capture ends the worker. The receiver's host name is
/// resolved at each attempt, so a name that does not resolve (yet) is a
/// wait like a receiver that is not listening. While the receiver has
/// paused the stream the TV capture is closed.
async fn run(args: SlimArgs, link: Link) -> Result<()> {
    let mut receiver = Resolver::new(args.receiver);
    let (sink, flow) = Uplink::new(move || RemoteSender::connect(receiver.resolve()?));
    let video = VideoSignal::default();
    let capture = WebosCapture::new(video.clone());
    let source = RequestedCapture::new(flow.clone(), move |settings| {
        Ok(Box::new(capture.start(&settings)?) as Box<dyn FrameSource>)
    });
    let stopped = link.stop.stopped();
    Forwarder::new(source, sink)
        .surface(ForwarderLink::new(link, video, flow))
        .stop_deadline(STOP_DEADLINE)
        .run(stopped)
        .await
}

#[cfg(test)]
mod tests;
