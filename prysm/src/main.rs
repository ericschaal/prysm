mod ctrl_c;
mod stream;

use anyhow::Result;
use desktop_renderer::DesktopRenderer;
use prysm_capture::{PrysmCapturer};
use prysm_processor::PrysmProcessor;
use prysm_render::PrysmRenderer;
use v4l_capturer::V4lCapturer;
use crate::ctrl_c::CtrlCHandler;

const CAPTURE_WIDTH: u32 = 1920;
const CAPTURE_HEIGHT: u32 = 1080;

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    run_app().await
}

async fn run_app() -> Result<()> {
    // Create shutdown signal (watch channel broadcasts to all receivers)
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let _ = CtrlCHandler::new(shutdown_tx.clone()).start();

    // Pass shutdown receiver to capturer constructor
    let capturer = V4lCapturer::new("/dev/video2", shutdown_rx.clone())?;
    let processor = PrysmProcessor::default();

    // Pass shutdown sender and receiver to renderer constructor
    let renderer = DesktopRenderer::new(
        CAPTURE_WIDTH as usize,
        CAPTURE_HEIGHT as usize,
        shutdown_tx,
        shutdown_rx);

    let video_feed = capturer.run(CAPTURE_WIDTH, CAPTURE_HEIGHT);

    let (processor_stream, renderer_stream) = stream::stream_split(video_feed);

    let regions = processor.run(processor_stream);

    // This blocks until the window is closed
    renderer.with_frame_stream(renderer_stream).run(regions);


    Ok(())
}