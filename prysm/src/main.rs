mod stream;

use anyhow::Result;
use desktop_renderer::DesktopRendererBuilder;
use prysm_capture::{Frame, PrysmCapturer};
use prysm_core::EdgeSpectra;
use prysm_processor::PrysmProcessor;
use tokio_util::sync::CancellationToken;
use v4l_capturer::V4lCapturer;

const CAPTURE_WIDTH: u32 = 1920;
const CAPTURE_HEIGHT: u32 = 1080;

fn main() -> Result<()> {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    // Create shutdown token
    let shutdown_token = CancellationToken::new();

    let dummy_spectrum = EdgeSpectra::dummy(CAPTURE_WIDTH as usize, CAPTURE_HEIGHT as usize);
    let spectra = stream::StreamWatcher::new(dummy_spectrum);
    let dummy_frame = Frame::dummy(CAPTURE_WIDTH, CAPTURE_HEIGHT);
    let frames = stream::StreamWatcher::new(dummy_frame);

    // Configure renderer layout with target FPS from core config
    let config = prysm_core::Config::default();

    // Spawn dedicated runtime thread for all async work
    let runtime_handle = std::thread::spawn({
        // Clone what we need for the async runtime
        let shutdown_token = shutdown_token.clone();
        let spectra = spectra.clone();
        let frames = frames.clone();
        let config = config.clone();

        move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to build tokio runtime");

            rt.block_on(async move {
                let capturer = V4lCapturer::new("/dev/video0", shutdown_token.clone())
                    .expect("Failed to create V4L capturer");
                let processor = PrysmProcessor::new(&config);

                // Create async streams
                let video_feed = capturer.into_stream(CAPTURE_WIDTH, CAPTURE_HEIGHT);
                let (frame_stream, frame_stream_bis) = stream::stream_split(video_feed);
                let spectrum_stream = processor.into_stream(frame_stream);

                let spectrum_task = spectra.into_task(spectrum_stream);
                let frame_task = frames.into_task(frame_stream_bis);

                // Spawn ctrl-C handler
                let shutdown_token_clone = shutdown_token.clone();
                tokio::spawn(async move {
                    if tokio::signal::ctrl_c().await.is_ok() {
                        tracing::info!("Received Ctrl+C, initiating shutdown...");
                        shutdown_token_clone.cancel();
                    }
                });

                // Wait for shutdown signal
                shutdown_token.cancelled().await;
                tracing::info!("Runtime thread received shutdown signal");

                // Graceful shutdown: wait for stream tasks to finish
                let _ = tokio::join!(spectrum_task, frame_task);

                tracing::info!("Runtime thread shutting down cleanly");
            });
        }
    });

    let layout_config = desktop_renderer::LayoutConfig {
        target_fps: config.target_fps,
        ..Default::default()
    };

    let app = DesktopRendererBuilder::new(spectra.receiver())
        .with_shutdown_token(&shutdown_token)
        .with_frame_rx(frames.receiver())
        .with_layout(layout_config)
        .build();

    // Run desktop renderer on main thread (blocking until window closes)
    let result = desktop_renderer::run(app, &shutdown_token);

    // Wait for runtime thread to finish
    tracing::info!("Waiting for runtime thread to finish");
    runtime_handle.join().expect("Runtime thread panicked");

    tracing::info!("Application shutdown complete");

    result.map_err(|e| anyhow::anyhow!("Desktop renderer error: {e}"))
}
