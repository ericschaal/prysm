
use futures::StreamExt;
use anyhow::Result;
use desktop_renderer::DesktopRenderer;
use prysm_capture::{Frame, PrysmCapturer};
use prysm_processor::PrysmProcessor;
use prysm_render::PrysmRenderer;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use v4l_capturer::V4lCapturer;

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    // Leak capturer and processor to make them 'static
    // This is acceptable since they need to live for the entire program duration
    let capturer = Box::leak(Box::new(V4lCapturer::new("/dev/video1")?));
    let processor = Box::leak(Box::new(PrysmProcessor::default()));
    let renderer = DesktopRenderer::new();

    let video_feed = capturer.run(1920, 1080);

    // Create broadcast channel for frame distribution
    let (frame_tx, _) = broadcast::channel::<Frame>(10);

    // Subscribe to broadcast for processor and renderer
    let processor_rx = frame_tx.subscribe();
    let renderer_rx = frame_tx.subscribe();

    // Spawn task to broadcast frames
    tokio::spawn(async move {
        use futures::StreamExt;
        futures::pin_mut!(video_feed);
        while let Some(frame) = video_feed.next().await {
            // Ignore send errors (no active receivers)
            let _ = frame_tx.send(frame);
        }
    });

    // Convert broadcast receiver to stream for processor
    let processor_stream = BroadcastStream::new(processor_rx)
        .filter_map(|result| async move { result.ok() });

    let regions = processor.run(processor_stream);

    // This blocks until the window is closed
    renderer.with_frames(renderer_rx).run(regions);

    Ok(())
}
