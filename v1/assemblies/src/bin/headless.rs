//! Device capture host with owned file settings and WLED or terminal output.
use futures::{StreamExt, stream};
use prysm_assemblies::server::{HostEvent, run_headless, run_terminal};
use prysm_contract::application::LightingTarget;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let path = args
        .next()
        .ok_or("usage: prysm-headless <settings.json> [--terminal] [--start]")?;
    if path == "--help" || path == "-h" {
        println!(
            "usage: prysm-headless <settings.json> [--terminal] [--start]\nStarts idle; --start requests processed lighting. --terminal selects terminal output instead of WLED. SIGINT/SIGTERM shuts down."
        );
        return Ok(());
    }
    let (mut start, mut terminal) = (false, false);
    for flag in args {
        match flag.to_str() {
            Some("--start") if !start => start = true,
            Some("--terminal") if !terminal => terminal = true,
            _ => return Err("expected --start or --terminal, each at most once".into()),
        }
    }
    let path = std::path::PathBuf::from(path);
    #[cfg(unix)]
    let shutdown = {
        use tokio::signal::unix::{SignalKind, signal};
        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut terminate = signal(SignalKind::terminate())?;
        async move {
            tokio::select! {
                _ = interrupt.recv() => {},
                _ = terminate.recv() => {},
            }
        }
    };
    #[cfg(not(unix))]
    let shutdown = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            eprintln!("shutdown signal failed: {error}");
        }
    };
    let commands =
        stream::iter(start.then_some(LightingTarget::Processed)).chain(stream::pending());
    let report = |event| match event {
        HostEvent::Ready(instance) => eprintln!("ready (idle), instance {}", instance.0),
        HostEvent::Stopping => eprintln!("Stopping; awaiting capture and output cleanup."),
        HostEvent::Application(update) => eprintln!("{update:?}"),
        HostEvent::Accepted(accepted) => eprintln!("accepted {:?}", accepted.operation),
        HostEvent::CommandRejected(failure) | HostEvent::Failure(failure) => eprintln!("{failure}"),
        HostEvent::CleanupOverdue { pending } => eprintln!(
            "cleanup unconfirmed; still waiting for {}",
            pending.join(", ")
        ),
    };
    if terminal {
        run_terminal(path, commands, shutdown, report).await?;
    } else {
        run_headless(path, commands, shutdown, report).await?;
    }
    eprintln!("Stopped; all owners joined.");
    Ok(())
}
