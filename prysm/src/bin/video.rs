use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use prysm::video::{DEFAULT_VIDEO, video_feed};

#[derive(Parser)]
#[command(about = "Play a video through the desktop visualizer using FFmpeg 9+")]
struct Args {
    /// Video file to play
    #[arg(default_value = DEFAULT_VIDEO)]
    path: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();
    prysm::run(move |shutdown| video_feed(&args.path, shutdown))
}
