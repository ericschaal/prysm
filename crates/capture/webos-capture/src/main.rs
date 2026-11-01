//! Native webOS capture sender. Hardware resources never cross threads.
#![deny(unsafe_code)]
mod capture;
mod cli;
#[allow(unsafe_code)]
mod native;
mod pixels;
mod session;
mod shutdown;
mod transport;

use anyhow::{Result, ensure};
use clap::Parser;
use cli::Cli;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut events = native::events::Events::from_environment()?;
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
    let _metrics = cli.metrics_listen.and_then(|address| {
        match prysm_metrics::start(address, prysm_metrics::Role::Capturer) {
            Ok(exporter) => {
                tracing::info!(address = %exporter.address(), "metrics listening");
                Some(exporter)
            }
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), "metrics disabled");
                None
            }
        }
    });
    let mut session = match session::Session::connect(&cli) {
        Ok(session) => session,
        Err(error) => {
            events.send(&native::events::Event::Disconnected)?;
            return Err(error);
        }
    };
    events.send(&native::events::Event::Connected)?;
    let capturing = session
        .wait_for_capture(&cli, &stop)
        .unwrap_or_else(|error| {
            tracing::info!(error = %format!("{error:#}"), "idle receiver disconnected");
            false
        });
    if !capturing {
        events.send(&native::events::Event::Disconnected)?;
        return Ok(());
    }
    let mut ui = Some(capture::NativeUi::new(cli.source));
    let result = session.configure(&mut ui).and_then(|()| {
        events.send(&native::events::Event::Capturing)?;
        session.run(&stop, &mut native::Video::open, &mut ui)
    });
    match &result {
        Ok(()) => events.send(&native::events::Event::Disconnected)?,
        Err(error) => events.send(&native::events::Event::Failed {
            message: format!("{error:#}"),
        })?,
    }
    result
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
