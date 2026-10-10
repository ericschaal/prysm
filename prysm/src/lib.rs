mod stream;
pub mod video;

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
    S: Stream<Item = Result<Frame>> + Send + 'static,
{
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    // Create shutdown token
    let shutdown_token = CancellationToken::new();

    let config = prysm_core::Config::default();
    let (edge_colors_tx, edge_colors_rx) = tokio::sync::watch::channel(EdgeColors::black(
        CAPTURE_WIDTH as usize,
        CAPTURE_HEIGHT as usize,
        config.sample_density,
    ));
    let dummy_frame = Frame::dummy(CAPTURE_WIDTH, CAPTURE_HEIGHT);
    let (frames_tx, frames_rx) = tokio::sync::watch::channel(dummy_frame);

    // Spawn dedicated runtime thread for all async work
    let runtime_handle = std::thread::spawn({
        // Clone what we need for the async runtime
        let shutdown_token = shutdown_token.clone();

        move || {
            let _shutdown_guard = shutdown_token.clone().drop_guard();
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("Failed to build tokio runtime")?;

            rt.block_on(async move {
                let video_feed = create_feed(shutdown_token.clone())?;
                let processor = PrysmProcessor::new(&config);

                tokio::select! {
                    result = stream::publish_frames(
                        video_feed, processor, frames_tx, edge_colors_tx, &shutdown_token,
                    ) => result?,
                    result = tokio::signal::ctrl_c() => {
                        result.context("Failed to listen for Ctrl+C")?;
                        tracing::info!("Received Ctrl+C, initiating shutdown...");
                        shutdown_token.cancel();
                    }
                }
                tracing::info!("Runtime thread shutting down cleanly");
                Ok::<(), anyhow::Error>(())
            })
        }
    });

    let app = DesktopRendererBuilder::new(edge_colors_rx)
        .with_shutdown_token(&shutdown_token)
        .with_frame_rx(frames_rx)
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
