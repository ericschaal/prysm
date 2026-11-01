//! Native webOS capture sender. Hardware resources never cross threads.
#![deny(unsafe_code)]
mod capture;
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
    ensure!(
        std::env::args_os().len() == 1,
        "capture worker accepts no command-line arguments"
    );
    let startup = WorkerConfig::decode(
        &std::env::var(WORKER_CONFIG_ENV)
            .context("capture worker requires supervisor configuration")?,
    )?;
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
    let _metrics = if startup.metrics {
        match prysm_metrics::start(([0, 0, 0, 0], 9101).into(), prysm_metrics::Role::Capturer) {
            Ok(exporter) => {
                tracing::info!(address = %exporter.address(), "metrics listening");
                Some(exporter)
            }
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), "metrics disabled");
                None
            }
        }
    } else {
        None
    };
    loop {
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            events.send(&events::Event::Disconnected)?;
            return Ok(());
        }
        let mut session = match session::Session::connect(&config) {
            Ok(session) => session,
            Err(error) => {
                events.send(&events::Event::Disconnected)?;
                return Err(error);
            }
        };
        events.send(&events::Event::Connected)?;
        let capturing = session
            .wait_for_capture(&config, &stop)
            .unwrap_or_else(|error| {
                tracing::info!(error = %format!("{error:#}"), "idle receiver disconnected");
                false
            });
        if !capturing || stop.load(std::sync::atomic::Ordering::Relaxed) {
            events.send(&events::Event::Disconnected)?;
            return Ok(());
        }
        let mut ui = Some(capture::NativeUi::new(config.source));
        let result = session.configure(&mut ui, &mut events).and_then(|()| {
            events.send(&events::Event::Capturing)?;
            session.run(
                &stop,
                &mut native::Video::open,
                &mut ui,
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
            drop(ui);
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

fn capture_lock(path: &std::path::Path) -> Result<nix::fcntl::Flock<std::fs::File>> {
    use anyhow::Context;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .context("cannot open capture lock")?;
    nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock)
        .map_err(|(_, error)| error)
        .context("another Prysm capturer is already running")
}

#[cfg(test)]
mod tests {
    #[test]
    fn capture_is_exclusive_until_the_previous_process_releases_its_lock() {
        let path = std::env::temp_dir().join(format!("prysm-capture-{}.lock", std::process::id()));
        let lock = super::capture_lock(&path).unwrap();
        assert!(super::capture_lock(&path).is_err());
        drop(lock);
        let lock = super::capture_lock(&path).unwrap();
        drop(lock);
        std::fs::remove_file(path).unwrap();
    }
}
