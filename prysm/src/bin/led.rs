use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use clap::Parser;
use futures::{Stream, StreamExt};
use led_renderer::WledRenderer;
use prysm::video::{DEFAULT_VIDEO, video_feed};
use prysm::{CAPTURE_HEIGHT, CAPTURE_WIDTH};
use prysm_capture::{Capturer, PrysmCapturer};
use prysm_core::{Config, EdgeColors};
use prysm_processor::PrysmProcessor;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Parser)]
#[command(about = "Stream camera or FFmpeg video colors to WLED over DDP")]
struct Args {
    /// WLED hostname or IP with port (normally 4048)
    #[arg(value_name = "WLED_ADDRESS")]
    address: String,

    /// Top edge LED count, left-to-right as viewed from the front
    top: u16,
    /// Right edge LED count, top-to-bottom
    right: u16,
    /// Bottom edge LED count, right-to-left
    bottom: u16,
    /// Left edge LED count, bottom-to-top
    left: u16,

    /// Linux video device path or macOS `AVFoundation` device UID
    device: Option<String>,

    /// Play a video with `FFmpeg` 9+ (defaults to the local Philips HDR test clip)
    #[arg(long, value_name = "PATH", num_args = 0..=1, default_missing_value = DEFAULT_VIDEO, conflicts_with = "device")]
    video: Option<PathBuf>,
}

impl Args {
    fn led_counts(&self) -> Result<[usize; 4]> {
        let led_counts = [self.top, self.right, self.bottom, self.left].map(usize::from);
        let total: usize = led_counts.iter().sum();
        ensure!(
            (1..=usize::from(u16::MAX)).contains(&total),
            "Total LED count must be between 1 and 65535"
        );
        Ok(led_counts)
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = Args::parse();
    let led_counts = args.led_counts()?;
    tracing::subscriber::set_global_default(tracing_subscriber::FmtSubscriber::new())?;

    let mut renderer = WledRenderer::new(args.address.as_str())
        .with_context(|| format!("Failed to connect to WLED at {}", args.address))?;
    let shutdown = CancellationToken::new();
    let _shutdown_guard = shutdown.clone().drop_guard();
    let frames = match args.video {
        None => Capturer::new(args.device.as_deref(), shutdown.clone())?
            .into_stream(CAPTURE_WIDTH, CAPTURE_HEIGHT)
            .boxed(),
        Some(path) => video_feed(&path, shutdown.clone())?.boxed(),
    };
    let colors = PrysmProcessor::new(&Config::default()).into_stream(frames);
    tracing::info!(address = %args.address, ?led_counts, "Streaming to WLED");

    tokio::select! {
        result = tokio::signal::ctrl_c() => {
            result.context("Failed to listen for Ctrl+C")?;
            tracing::info!("Received Ctrl+C, stopping WLED stream");
            Ok(())
        }
        result = render_stream(colors, &mut renderer, led_counts, &shutdown) => result,
    }
}

async fn render_stream(
    colors: impl Stream<Item = EdgeColors>,
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
                    ensure!(shutdown.is_cancelled(), "Capture stream ended unexpectedly");
                    return Ok(());
                };
                renderer.render_edges(&colors, led_counts).context("Failed to send WLED frame")?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_capture::{Frame, PixelFormat};
    use std::net::UdpSocket;
    use std::time::Duration;

    fn parse(args: &[&str]) -> Result<Args> {
        let args = Args::try_parse_from(std::iter::once("led").chain(args.iter().copied()))?;
        args.led_counts()?;
        Ok(args)
    }

    #[test]
    fn parses_layout_optional_device_and_help() {
        let args = parse(&["wled.local:4048", "96", "54", "96", "0", "/dev/video2"]).unwrap();
        assert_eq!(args.address, "wled.local:4048");
        assert_eq!(args.led_counts().unwrap(), [96, 54, 96, 0]);
        assert_eq!(args.device.as_deref(), Some("/dev/video2"));
        assert!(args.video.is_none());
        assert!(
            parse(&["wled.local:4048", "1", "0", "0", "0"])
                .unwrap()
                .device
                .is_none()
        );
        for flag in ["--help", "-h"] {
            assert_eq!(
                Args::try_parse_from(["led", flag]).unwrap_err().kind(),
                clap::error::ErrorKind::DisplayHelp
            );
        }
    }

    #[test]
    fn parses_video_with_default_or_explicit_path() {
        for (extra, expected) in [
            (vec!["--video"], DEFAULT_VIDEO),
            (vec!["--video", "/tmp/my video.mp4"], "/tmp/my video.mp4"),
        ] {
            let mut args = vec!["wled.local:4048", "96", "54", "96", "54"];
            args.extend(extra);
            assert_eq!(parse(&args).unwrap().video, Some(PathBuf::from(expected)));
        }
        assert!(parse(&["host", "1", "1", "1", "1", "--video", "clip.mp4", "device"]).is_err());
    }

    #[test]
    fn rejects_bad_arguments_and_invalid_totals() {
        for args in [
            vec![],
            vec!["wled.local:4048"],
            vec!["host", "1", "1", "1", "1", "device", "extra"],
            vec!["host", "0", "0", "0", "0"],
            vec!["host", "65535", "1", "0", "0"],
            vec!["host", "65536", "0", "0", "0"],
            vec!["host", "-1", "0", "0", "0"],
            vec!["host", "red", "0", "0", "0"],
            vec!["host", "1", "1", "1", "1", "--unknown"],
            vec!["host", "1", "1", "1", "1", "device", "--video"],
        ] {
            assert!(parse(&args).is_err(), "accepted {args:?}");
        }
        assert!(parse(&["host", "65535", "0", "0", "0"]).is_ok());
    }

    #[tokio::test]
    async fn processes_a_frame_sends_colors_and_reports_capture_end() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut renderer = WledRenderer::new(socket.local_addr().unwrap()).unwrap();
        let frames = futures::stream::iter([Frame::fill(255, 2, 2, PixelFormat::RGB24)]);
        let config = Config {
            brightness_percent: 100,
            smoothing_seconds: 0.0,
            ..Config::default()
        };
        let colors = PrysmProcessor::new(&config).into_stream(frames);
        let result = render_stream(colors, &mut renderer, [1; 4], &CancellationToken::new()).await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Capture stream ended unexpectedly")
        );
        let mut bytes = [0u8; 1500];
        let size = socket.recv(&mut bytes).unwrap();
        assert_eq!(size, 22);
        assert_eq!(&bytes[..4], &[0x41, 1, 0x0b, 1]);
        assert_eq!(&bytes[10..size], &[255; 12]);
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
