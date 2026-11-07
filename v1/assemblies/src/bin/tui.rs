//! Device capture to the interactive terminal preview and command interface.
mod common;

use futures::stream;
use prysm_assemblies::server::run_tui;
use prysm_contract::application::LightingTarget;

const USAGE: &str = "usage: prysm-tui <settings.json> [--start]
Device capture to the terminal; output settings require leds.
Press : for on, off, or load <path>. Stdin/stdout must be interactive.
Starts idle; --start requests processed lighting. SIGINT/SIGTERM shuts down.";

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Some(args) = common::arguments(USAGE)? else {
        return Ok(());
    };
    let shutdown = common::shutdown()?;
    let commands = stream::iter(args.start.then_some(LightingTarget::Processed));
    run_tui(args.settings, commands, shutdown, common::report).await?;
    eprintln!("Stopped; all owners joined.");
    Ok(())
}
