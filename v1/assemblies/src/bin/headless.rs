//! Device capture to WLED, with diagnostics on stderr.
mod common;

use futures::stream;
use prysm_assemblies::headless;
use prysm_contract::application::LightingTarget;

const USAGE: &str = "usage: prysm-headless <settings.json> [--start]
Device capture to WLED; output settings require address and leds.
Starts idle; --start requests processed lighting. SIGINT/SIGTERM shuts down.";

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Some(args) = common::arguments(USAGE)? else {
        return Ok(());
    };
    let shutdown = common::shutdown()?;
    let commands = stream::iter(args.start.then_some(LightingTarget::Processed));
    headless::run(args.settings, commands, shutdown, common::report).await?;
    eprintln!("Stopped; all owners joined.");
    Ok(())
}
