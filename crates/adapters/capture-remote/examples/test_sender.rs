//! A stand-in for the TV's slim worker, for end-to-end tests: streams
//! generated frames to a Prysm portal's capture stream path.
//!
//! ```sh
//! cargo run -p prysm-capture-remote --example test_sender -- \
//!     ws://127.0.0.1:8787/api/v1/capture/stream [--refuse REASON] [--stop-after N] \
//!     [--offer JSON]
//! ```
//!
//! It connects again after each connection ends, however it ended (unlike
//! the TV, which stops once replaced), every 100 ms while the portal answers
//! `503` (no remote source receives: lighting off).
//! `--refuse REASON` refuses every connection with `REASON`, as a TV whose
//! capture is busy does: each refusal fails the receiving source, and a
//! source opened again (lighting retried) is refused too. `--stop-after N`
//! exits after `N` frames. Each connection prints one line to stdout
//! (`connected WxH`, `refused` once Prysm has ended the refused connection),
//! so a test can wait for it. Runs until killed otherwise. `--offer` announces
//! an [`Offer`] of those encodings' modes (`{ "raw": [Mode, ...] }`) instead
//! of every request the portal accepts in every encoding, as a TV with fewer
//! encoders does.
use prysm_capture_remote::{Offer, RemoteSender};
use prysm_core::{Frame, PixelFormat};
use prysm_ports::StreamEncoding;
use prysm_ports::{FrameSink, Mode};
use std::collections::BTreeMap;
use std::io::Write;
use std::net::ToSocketAddrs;
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::http::Uri;

const RETRY: Duration = Duration::from_millis(100);

/// How often a refused connection is checked for Prysm's end of it.
const POLL: Duration = Duration::from_millis(10);

struct Options {
    url: Uri,
    refuse: Option<String>,
    stop_after: Option<u64>,
    offer: Offer,
}

fn options() -> Result<Options, String> {
    let mut args = std::env::args().skip(1);
    let mut url = None;
    let mut refuse = None;
    let mut stop_after = None;
    let mut offer = prysm_capture_remote::testkit::offer();
    fn number<T: std::str::FromStr>(value: Option<String>, name: &str) -> Result<T, String> {
        value
            .and_then(|value| value.parse().ok())
            .ok_or(format!("{name} takes a number"))
    }
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--refuse" => refuse = Some(args.next().ok_or("--refuse takes a reason")?),
            "--stop-after" => stop_after = Some(number(args.next(), "--stop-after")?),
            "--offer" => {
                let json = args.next().ok_or("--offer takes JSON")?;
                let encodings: BTreeMap<StreamEncoding, Vec<Mode>> =
                    serde_json::from_str(&json).map_err(|error| format!("--offer: {error}"))?;
                offer = Offer::new(encodings);
            }
            _ if url.is_none() => {
                url = Some(arg.parse().map_err(|error| format!("{arg}: {error}"))?);
            }
            _ => return Err(format!("unexpected argument {arg}")),
        }
    }
    Ok(Options {
        url: url
            .ok_or("usage: test_sender URL [--refuse REASON] [--stop-after N] [--offer JSON]")?,
        refuse,
        stop_after,
        offer,
    })
}

/// One flat colour per frame, cycling slowly through red, green and blue.
fn frame(index: u64, width: u32, height: u32) -> Frame {
    let phase = (index / 30 % 3) as usize;
    let mut pixel = [16u8; 3];
    pixel[phase] = 220;
    let data = pixel.repeat((width * height) as usize);
    Frame::new(data, width, height, PixelFormat::RGB24).expect("a valid frame")
}

fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn main() -> ExitCode {
    let options = match options() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let host = options.url.host().unwrap_or("127.0.0.1");
    let port = options.url.port_u16().unwrap_or(80);
    let Some(address) = (host, port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut all| all.next())
    else {
        eprintln!("cannot resolve {host}:{port}");
        return ExitCode::from(2);
    };
    let mut sent = 0u64;
    loop {
        let Ok(mut sender) = RemoteSender::connect(&options.url, address, &options.offer) else {
            thread::sleep(RETRY);
            continue;
        };
        if let Some(reason) = &options.refuse {
            sender.refuse(reason);
            // Connect again only once Prysm has ended this connection: it
            // takes a newer one over one it has not read yet, so the
            // refusal would be lost. The TV waits a second instead.
            while sender.tick().is_ok() {
                thread::sleep(POLL);
            }
            say("refused");
            continue;
        }
        let request = sender.request();
        say(&format!("connected {}x{}", request.width, request.height));
        let interval = Duration::from_secs(1) / request.fps;
        let mut next = Instant::now();
        loop {
            if sender
                .deliver(&frame(sent, request.width, request.height))
                .is_err()
            {
                break;
            }
            sent += 1;
            if options.stop_after.is_some_and(|stop| sent >= stop) {
                say("stopped");
                return ExitCode::SUCCESS;
            }
            next += interval;
            thread::sleep(next.saturating_duration_since(Instant::now()));
        }
    }
}
