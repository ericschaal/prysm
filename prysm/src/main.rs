
use anyhow::Result;
use desktop_renderer::DesktopRenderer;
use prysm_capture::PrysmCapturer;
use prysm_processor::PrysmProcessor;
use prysm_render::PrysmRenderer;
use v4l_capturer::V4lCapturer;

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    // Leak capturer and processor to make them 'static
    // This is acceptable since they need to live for the entire program duration
    let capturer = Box::leak(Box::new(V4lCapturer::new("/dev/video0")?));
    let processor = Box::leak(Box::new(PrysmProcessor::default()));
    let mut renderer = DesktopRenderer::new();

    let video_feed = capturer.run(1920, 1080);
    let regions = processor.run(video_feed);

    // This blocks until the window is closed
    renderer.run(regions);

    Ok(())
}
