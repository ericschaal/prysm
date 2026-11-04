//! Native webOS capture sender. Hardware resources never cross threads.
#![deny(unsafe_code)]
mod capture;
mod lock;
mod metrics;
use lock::capture_lock;
mod config;
mod control;
mod encoder;
#[allow(unsafe_code)]
mod events;
#[allow(unsafe_code)]
mod native;
mod pixels;
mod session;
mod shutdown;
mod transport;

use anyhow::{Context, Result, ensure};
use config::Config;
use prysm_webos_protocol::{WORKER_CONFIG_ENV, WorkerConfig};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if let [flag, value] = args.as_slice()
        && flag == "--verify-instance"
    {
        let instance = value.to_str().context("invalid instance URL")?.parse()?;
        transport::Sender::connect(&instance)?;
        return Ok(());
    }
    ensure!(
        args.is_empty(),
        "capture worker accepts no command-line arguments"
    );
    run_worker()
}

fn run_worker() -> Result<()> {
    let startup = WorkerConfig::decode(
        &std::env::var(WORKER_CONFIG_ENV)
            .context("capture worker requires supervisor configuration")?,
    )?;
    let metrics = startup.metrics;
    let mut config = Config::from(startup);
    let mut events = events::Events::from_environment()?;
    let mut control = control::Control::from_stdin()?;
    tracing_subscriber::fmt()
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_writer(std::io::stderr)
        .init();
    ensure!(cfg!(target_os = "linux"), "capture requires webOS Linux");
    ensure!(
        nix::unistd::Uid::effective().is_root(),
        "TV capture requires root"
    );
    let _lock = capture_lock(std::path::Path::new("/tmp/prysm-capture.lock"))?;
    let (stop, _signal_waiter) = shutdown::listen()?;
    let _metrics = metrics::exporter(metrics);
    loop {
        if !control.wait_for_video(&stop)? {
            events.send(&events::Event::Disconnected)?;
            return Ok(());
        }
        let mut session = match session::Session::connect(&config) {
            Ok(session) => session,
            Err(error) => {
                tracing::info!(error = %format!("{error:#}"), "waiting for Prysm");
                events.send(&events::Event::Disconnected)?;
                return Ok(());
            }
        };
        if session.busy() {
            events.send(&events::Event::ReceiverBusy)?;
            return Ok(());
        }
        events.send(&events::Event::Connected)?;
        let capturing = match session.wait_for_capture(&config, || control.video_ready(&stop)) {
            Ok(capturing) => capturing,
            Err(error) if error.is::<transport::ReceiverBusy>() => {
                events.send(&events::Event::ReceiverBusy)?;
                return Ok(());
            }
            Err(error) => {
                tracing::info!(error = %format!("{error:#}"), "idle receiver disconnected");
                false
            }
        };
        if !capturing || stop.load(std::sync::atomic::Ordering::Relaxed) {
            events.send(&events::Event::Disconnected)?;
            return Ok(());
        }
        let result = session.configure(&mut events).and_then(|()| {
            events.send(&events::Event::Capturing)?;
            session.run(
                &stop,
                &mut |options| native::Video::open(options, config.source),
                &mut || control.poll(),
                &mut events,
            )
        });
        if config.compression == config::Compression::H264Hardware
            && result
                .as_ref()
                .is_err_and(|error| error.downcast_ref::<encoder::HardwareFailed>().is_some())
            && !stop.load(std::sync::atomic::Ordering::Relaxed)
        {
            tracing::warn!("hardware encoder failed; reconnecting with OpenH264");
            events.send(&events::Event::VideoResetting)?;
            drop(session);
            events.send(&events::Event::VideoReset)?;
            config.compression = config::Compression::H264Software;
            continue;
        }
        match &result {
            Ok(()) => events.send(&events::Event::Disconnected)?,
            Err(error) => events.send(&events::Event::Failed {
                message: format!("{error:#}"),
            })?,
        }
        return result;
    }
}
