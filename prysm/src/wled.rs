//! Headless processing and WLED output on a single-threaded Tokio runtime.

use std::num::NonZeroU32;

use anyhow::{Context, Result};
use futures::{Stream, StreamExt};
use led_renderer::WledRenderer;
use prysm_core::{Config, EdgeColors};
use prysm_processor::PrysmProcessor;
use tokio_util::sync::CancellationToken;

use crate::config::{Input, LedCounts, WledAddress};

pub(super) fn run(
    input: Input,
    settings: Config,
    address: WledAddress,
    refresh_rate_hz: NonZeroU32,
    leds: LedCounts,
) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("Failed to build tokio runtime")?;
    runtime.block_on(async move {
        let mut renderer = WledRenderer::new(address.as_str())
            .with_context(|| format!("Failed to connect to WLED at {}", address.as_str()))?;
        renderer.set_refresh_rate_hz(refresh_rate_hz);
        let shutdown = CancellationToken::new();
        let _shutdown_guard = shutdown.clone().drop_guard();
        let frames = input.into_stream(settings.capture_resolution, shutdown.clone())?;
        let colors = PrysmProcessor::new(&settings).into_stream(frames);
        let counts = leds.counts();
        tracing::info!(address = address.as_str(), ?counts, "Streaming to WLED");
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                result.context("Failed to listen for Ctrl+C")?;
                tracing::info!("Received Ctrl+C, stopping WLED stream");
                Ok(())
            }
            result = render_stream(colors, &mut renderer, counts, &shutdown) => result,
        }
    })
}

async fn render_stream(
    colors: impl Stream<Item = Result<EdgeColors>>,
    renderer: &mut WledRenderer,
    led_counts: [usize; 4],
    shutdown: &CancellationToken,
) -> Result<()> {
    futures::pin_mut!(colors);
    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => return Ok(()),
            colors = colors.next() => {
                let Some(colors) = colors else {
                    return Ok(());
                };
                let colors = colors?;
                renderer.render_edges(&colors, led_counts).context("Failed to send WLED frame")?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_capture::{Frame, PixelFormat};
    use prysm_core::{BrightnessPercent, SmoothingSeconds};
    use std::{net::UdpSocket, time::Duration};

    #[tokio::test]
    async fn processes_a_frame_sends_colors_and_completes_at_eof() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut renderer = WledRenderer::new(socket.local_addr().unwrap()).unwrap();
        let frames = futures::stream::iter([Ok(Frame::fill(255, 2, 2, PixelFormat::RGB24))]);
        let config = Config {
            brightness_percent: BrightnessPercent(100),
            smoothing_seconds: SmoothingSeconds(0.0),
            ..Config::default()
        };
        let colors = PrysmProcessor::new(&config).into_stream(frames);
        let result = render_stream(colors, &mut renderer, [1; 4], &CancellationToken::new()).await;
        result.unwrap();
        let mut bytes = [0u8; 1500];
        let size = socket.recv(&mut bytes).unwrap();
        assert_eq!(size, 22);
        assert_eq!(&bytes[..4], &[0x41, 1, 0x0b, 1]);
        assert_eq!(&bytes[10..size], &[255; 12]);
    }

    #[tokio::test]
    async fn source_error_survives_processing_and_wled_output() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut renderer = WledRenderer::new(socket.local_addr().unwrap()).unwrap();
        let frames = futures::stream::iter([
            Ok(Frame::fill(255, 2, 2, PixelFormat::RGB24)),
            Err(anyhow::anyhow!("decoder failed").context("video input")),
        ]);
        let colors = PrysmProcessor::default().into_stream(frames);
        let error = render_stream(colors, &mut renderer, [1; 4], &CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(format!("{error:#}"), "video input: decoder failed");
    }

    #[tokio::test]
    async fn cancellation_stops_without_waiting_for_capture() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut renderer = WledRenderer::new(socket.local_addr().unwrap()).unwrap();
        let shutdown = CancellationToken::new();
        shutdown.cancel();
        render_stream(futures::stream::pending(), &mut renderer, [1; 4], &shutdown)
            .await
            .unwrap();
        socket.set_nonblocking(true).unwrap();
        assert_eq!(
            socket.recv(&mut [0; 1500]).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[tokio::test]
    async fn video_eof_cancelling_during_poll_is_successful() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut renderer = WledRenderer::new(socket.local_addr().unwrap()).unwrap();
        let shutdown = CancellationToken::new();
        let colors = futures::stream::poll_fn(|_| {
            shutdown.cancel();
            std::task::Poll::Ready(None)
        });
        render_stream(colors, &mut renderer, [1; 4], &shutdown)
            .await
            .unwrap();
    }
}
