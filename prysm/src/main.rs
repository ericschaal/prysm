use futures::StreamExt;
use anyhow::Result;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use desktop_renderer::DesktopRenderer;
use prysm_capture::{Frame, PrysmCapturer};
use prysm_processor::PrysmProcessor;
use prysm_render::PrysmRenderer;
use v4l_capturer::V4lCapturer;

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    run_app().await
}

async fn run_app() -> Result<()> {
    // Create shutdown signal (watch channel broadcasts to all receivers)
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // Spawn Ctrl+C handler
    let shutdown_signal = shutdown_tx.clone();
    tokio::spawn(async move {
        match tokio::signal::ctrl_c().await {
            Ok(()) => {
                tracing::info!("Received Ctrl+C, initiating shutdown...");
                let _ = shutdown_signal.send(true);
            }
            Err(err) => {
                tracing::error!("Failed to listen for Ctrl+C: {}", err);
            }
        }
    });

    // Pass shutdown receiver to capturer constructor
    let capturer = V4lCapturer::new("/dev/video1", shutdown_rx.clone())?;
    let processor = PrysmProcessor::default();

    // Pass shutdown sender and receiver to renderer constructor
    let renderer = DesktopRenderer::new(shutdown_tx.clone(), shutdown_rx.clone());

    let video_feed = capturer.run(1920, 1080);

    // Create broadcast channel for frame distribution
    let (frame_tx, _) = broadcast::channel::<Frame>(10);

    // Subscribe to broadcast for processor and renderer
    let processor_rx = frame_tx.subscribe();
    let renderer_rx = frame_tx.subscribe();

    // Spawn task to broadcast frames
    let broadcaster = tokio::spawn(async move {
        futures::pin_mut!(video_feed);
        while let Some(frame) = video_feed.next().await {
            let _ = frame_tx.send(frame);
        }
    });

    // Convert broadcast receiver to stream for processor
    let processor_stream = BroadcastStream::new(processor_rx)
        .filter_map(|result| async move { result.ok() });
    let renderer_stream = BroadcastStream::new(renderer_rx)
        .filter_map(|result| async move { result.ok() });

    let regions = processor.run(processor_stream);

    // This blocks until the window is closed
    renderer.with_frame_stream(renderer_stream).run(regions);

    // Optionally wait for broadcaster to finish (it won't until capturer stops)
    let _ = broadcaster.await;

    Ok(())
}