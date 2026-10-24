use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Process video from a device or file for WLED or an on-screen preview")]
struct Args {
    /// Runtime TOML configuration file
    #[arg(long, default_value = "prysm.toml", value_name = "PATH")]
    config: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();
    tracing::subscriber::set_global_default(tracing_subscriber::FmtSubscriber::new())?;
    prysm::run_from_config(&args.config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_default_or_custom_config_and_shows_help() {
        assert_eq!(
            Args::try_parse_from(["prysm"]).unwrap().config,
            PathBuf::from("prysm.toml")
        );
        assert_eq!(
            Args::try_parse_from(["prysm", "--config", "/tmp/my config.toml"])
                .unwrap()
                .config,
            PathBuf::from("/tmp/my config.toml")
        );
        assert!(Args::try_parse_from(["prysm", "--unknown"]).is_err());
        for flag in ["--help", "-h"] {
            assert_eq!(
                Args::try_parse_from(["prysm", flag]).unwrap_err().kind(),
                clap::error::ErrorKind::DisplayHelp
            );
        }
    }
}
