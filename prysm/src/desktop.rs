//! Desktop preview: keep the GUI on the main thread and processing on a runtime thread.

use crate::stream;
use anyhow::{Context, Result};
use desktop_renderer::{DesktopRendererBuilder, PreviewFrame};
use futures::Stream;
use prysm_capture::Frame;
use prysm_core::{Config, EdgeColors};
use prysm_processor::PrysmProcessor;
use std::num::NonZeroU32;
use tokio_util::sync::CancellationToken;

/// Run the desktop visualizer with a frame source created on the async runtime.
pub fn run<S>(
    config: Config,
    refresh_rate_hz: NonZeroU32,
    create_feed: impl FnOnce(CancellationToken) -> Result<S> + Send + 'static,
) -> Result<()>
where
    S: Stream<Item = Result<Frame>> + Send + 'static,
{
    let [width, height] = config.capture_resolution.dimensions();
    let shutdown_token = CancellationToken::new();

    let (edge_colors_tx, edge_colors_rx) = tokio::sync::watch::channel(EdgeColors::black(
        width as usize,
        height as usize,
        config.sample_density,
    ));
    let dummy_frame = Frame::dummy(width, height);
    let (frames_tx, frames_rx) = tokio::sync::watch::channel(PreviewFrame {
        frame: dummy_frame,
        viewport: None,
    });

    let runtime_handle = std::thread::spawn({
        let shutdown_token = shutdown_token.clone();

        move || {
            let _shutdown_guard = shutdown_token.clone().drop_guard();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("Failed to build tokio runtime")?;

            runtime.block_on(async move {
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
        .with_refresh_rate_hz(refresh_rate_hz)
        .with_shutdown_token(&shutdown_token)
        .with_frame_rx(frames_rx)
        .build();

    let result = desktop_renderer::run(app, &shutdown_token);

    tracing::info!("Waiting for runtime thread to finish");
    runtime_handle
        .join()
        .map_err(|_| anyhow::anyhow!("Runtime thread panicked"))??;

    tracing::info!("Application shutdown complete");

    // shortcut: eframe errors can be non-Send; retain their text until the app supports local errors.
    result.map_err(|e| anyhow::anyhow!("Desktop renderer error: {e}"))
}
