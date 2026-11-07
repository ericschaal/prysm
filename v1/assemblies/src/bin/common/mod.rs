//! Shared CLI mechanics. Each binary selects its concrete composition explicitly.
use prysm_assemblies::HostEvent;
use std::{future::Future, io, path::PathBuf};

pub struct Arguments {
    pub settings: PathBuf,
    pub start: bool,
}

pub fn arguments(usage: &str) -> Result<Option<Arguments>, Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let path = args.next().ok_or_else(|| usage.to_owned())?;
    if path == "--help" || path == "-h" {
        println!("{usage}");
        return Ok(None);
    }
    if path.as_encoded_bytes().starts_with(b"-") {
        return Err(format!("expected a settings file path\n{usage}").into());
    }
    let mut start = false;
    for flag in args {
        if flag == "--start" && !start {
            start = true;
        } else {
            return Err(format!("unexpected argument: {}\n{usage}", flag.to_string_lossy()).into());
        }
    }
    Ok(Some(Arguments {
        settings: path.into(),
        start,
    }))
}

/// Register signals before constructing owners, not when the returned future is polled.
pub fn shutdown() -> io::Result<impl Future<Output = ()> + Send> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut terminate = signal(SignalKind::terminate())?;
        Ok(async move {
            tokio::select! {
                _ = interrupt.recv() => {},
                _ = terminate.recv() => {},
            }
        })
    }
    #[cfg(not(unix))]
    Ok(async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            eprintln!("shutdown signal failed: {error}");
        }
    })
}

/// The TUI calls this only after its diagnostics sink closes.
pub fn report(event: HostEvent) {
    match event {
        HostEvent::Ready(instance) => eprintln!("ready (idle), instance {}", instance.0),
        HostEvent::Stopping => eprintln!("Stopping; awaiting capture and output cleanup."),
        HostEvent::Application(update) => eprintln!("{update:?}"),
        HostEvent::Accepted(accepted) => eprintln!("accepted {:?}", accepted.operation),
        HostEvent::CommandRejected(failure) | HostEvent::Failure(failure) => eprintln!("{failure}"),
        HostEvent::CleanupOverdue { pending } => eprintln!(
            "cleanup unconfirmed; still waiting for {}",
            pending.join(", ")
        ),
    }
}
