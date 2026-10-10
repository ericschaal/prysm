mod stream;

use anyhow::{Context, Result};
use desktop_renderer::DesktopRendererBuilder;
use futures::Stream;
use prysm_capture::Frame;
use prysm_core::EdgeColors;
use prysm_processor::PrysmProcessor;
use tokio_util::sync::CancellationToken;

// Low capture resolution on purpose: the default strip has ~38 horizontal
// samples, and the camera ISP's hardware downscale integrates every source pixel,
// which is both cheaper and more accurate than sampling a high-res frame.
pub const CAPTURE_WIDTH: u32 = 640;
pub const CAPTURE_HEIGHT: u32 = 360;

/// Run the desktop visualizer with a frame source created on the async runtime.
pub fn run<S>(
    create_feed: impl FnOnce(CancellationToken) -> Result<S> + Send + 'static,
) -> Result<()>
where
    S: Stream<Item = Frame> + Send + 'static,
{
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    // Create shutdown token
    let shutdown_token = CancellationToken::new();

    let config = prysm_core::Config::default();
    let edge_colors = stream::StreamWatcher::new(EdgeColors::black(
        CAPTURE_WIDTH as usize,
        CAPTURE_HEIGHT as usize,
        config.sample_density,
    ));
    let dummy_frame = Frame::dummy(CAPTURE_WIDTH, CAPTURE_HEIGHT);
    let frames = stream::StreamWatcher::new(dummy_frame);

    // Spawn dedicated runtime thread for all async work
    let runtime_handle = std::thread::spawn({
        // Clone what we need for the async runtime
        let shutdown_token = shutdown_token.clone();
        let edge_colors = edge_colors.clone();
        let frames = frames.clone();
        let config = config.clone();

        move || {
            let _shutdown_guard = shutdown_token.clone().drop_guard();
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("Failed to build tokio runtime")?;

            rt.block_on(async move {
                let video_feed = create_feed(shutdown_token.clone())?;
                let processor = PrysmProcessor::new(&config);

                // Create async streams
                let (frame_stream, frame_stream_bis) = stream::stream_split(video_feed);
                let edge_colors_stream = processor.into_stream(frame_stream);

                let edge_colors_task = edge_colors.into_task(edge_colors_stream);
                let frame_task = frames.into_task(frame_stream_bis);

                // Spawn ctrl-C handler
                let shutdown_token_clone = shutdown_token.clone();
                tokio::spawn(async move {
                    if tokio::signal::ctrl_c().await.is_ok() {
                        tracing::info!("Received Ctrl+C, initiating shutdown...");
                        shutdown_token_clone.cancel();
                    }
                });

                let result = stream::wait_for_shutdown(&shutdown_token, frame_task).await;
                let edge_colors_result =
                    edge_colors_task.await.context("Edge colors watcher failed");
                result?;
                edge_colors_result?;
                tracing::info!("Runtime thread shutting down cleanly");
                Ok::<(), anyhow::Error>(())
            })
        }
    });

    let app = DesktopRendererBuilder::new(edge_colors.receiver())
        .with_shutdown_token(&shutdown_token)
        .with_frame_rx(frames.receiver())
        .build();

    // Run desktop renderer on main thread (blocking until window closes)
    let result = desktop_renderer::run(app, &shutdown_token);

    // Wait for runtime thread to finish
    tracing::info!("Waiting for runtime thread to finish");
    runtime_handle
        .join()
        .map_err(|_| anyhow::anyhow!("Runtime thread panicked"))??;

    tracing::info!("Application shutdown complete");

    result.map_err(|e| anyhow::anyhow!("Desktop renderer error: {e}"))
}
