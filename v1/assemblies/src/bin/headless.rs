//! Minimal device-to-WLED host. No HTTP, stdin worker or settings persistence.
use futures::{StreamExt, stream};
use prysm_assemblies::{
    DeviceCaptureSettings, WledSettings,
    server::{HostEvent, TerminalSettings, run_headless, run_terminal},
};
use prysm_contract::application::{LightingTarget, Settings};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    capture: DeviceCaptureSettings,
    output: Output,
}

#[derive(Deserialize)]
#[serde(
    tag = "adapter",
    content = "settings",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum Output {
    Wled(WledSettings),
    Terminal(TerminalSettings),
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let path = args
        .next()
        .ok_or("usage: prysm-headless <config.json> [--start]")?;
    if path == "--help" || path == "-h" {
        println!(
            "usage: prysm-headless <config.json> [--start]\nStarts idle; --start requests processed lighting. SIGINT/SIGTERM shuts down."
        );
        return Ok(());
    }
    let start = match args.next() {
        Some(flag) if flag == "--start" => true,
        None => false,
        Some(_) => return Err("expected --start".into()),
    };
    if args.next().is_some() {
        return Err("unexpected extra arguments".into());
    }
    let config: Config = serde_json::from_slice(&std::fs::read(path)?)?;
    #[cfg(unix)]
    let shutdown = {
        use tokio::signal::unix::{SignalKind, signal};
        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut terminate = signal(SignalKind::terminate())?;
        async move {
            tokio::select! { _ = interrupt.recv() => {}, _ = terminate.recv() => {} }
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
    match config.output {
        Output::Wled(output) => {
            run_headless(settings(config.capture, output), commands, shutdown, report).await?
        }
        Output::Terminal(output) => {
            run_terminal(settings(config.capture, output), commands, shutdown, report).await?
        }
    };
    eprintln!("Stopped; all owners joined.");
    Ok(())
}

fn settings<O>(capture: DeviceCaptureSettings, output: O) -> Settings<DeviceCaptureSettings, O> {
    Settings {
        capture: Some(capture),
        output: Some(output),
        processing: Default::default(),
        profiles: vec![],
        active_profile: None,
    }
}
