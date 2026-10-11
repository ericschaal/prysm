use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use clap::Parser;
use futures::{Stream, StreamExt};
use led_renderer::WledRenderer;
use prysm::video::{DEFAULT_VIDEO, video_feed};
use prysm::{CAPTURE_HEIGHT, CAPTURE_WIDTH};
use prysm_capture::{Capturer, PrysmCapturer};
use prysm_core::{Config, EdgeColors};
use prysm_processor::PrysmProcessor;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Parser)]
#[command(
    about = "Stream camera or FFmpeg video colors to WLED over DDP",
    after_help = "Wiring starts at bottom-left: up the left edge, across the top, down the right, then back along the bottom (viewed from the front)."
)]
struct Args {
    /// Runtime TOML configuration file
    #[arg(long, default_value = "led.toml", value_name = "PATH")]
    config: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeConfig {
    wled_address: String,
    leds: LedCounts,
    source: Source,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LedCounts {
    top: u16,
    right: u16,
    bottom: u16,
    left: u16,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
enum Source {
    Camera {
        device: Option<String>,
    },
    Video {
        #[serde(default = "default_video_path")]
        path: PathBuf,
    },
}

fn default_video_path() -> PathBuf {
    PathBuf::from(DEFAULT_VIDEO)
}

impl RuntimeConfig {
    fn load(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("Cannot read configuration {}", path.display()))?;
        toml::from_str(&contents)
            .with_context(|| format!("Invalid configuration {}", path.display()))
    }

    fn led_counts(&self) -> Result<[usize; 4]> {
        let led_counts = [
            self.leds.top,
            self.leds.right,
            self.leds.bottom,
            self.leds.left,
        ]
        .map(usize::from);
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
    let config = RuntimeConfig::load(&args.config)?;
    let led_counts = config.led_counts()?;
    tracing::subscriber::set_global_default(tracing_subscriber::FmtSubscriber::new())?;

    let mut renderer = WledRenderer::new(config.wled_address.as_str())
        .with_context(|| format!("Failed to connect to WLED at {}", config.wled_address))?;
    let shutdown = CancellationToken::new();
    let _shutdown_guard = shutdown.clone().drop_guard();
    let frames = match config.source {
        Source::Camera { device } => Capturer::new(device.as_deref(), shutdown.clone())?
            .into_stream(CAPTURE_WIDTH, CAPTURE_HEIGHT)
            .map(|frame| frame.map_err(anyhow::Error::from))
            .boxed(),
        Source::Video { path } => video_feed(&path, shutdown.clone())?.boxed(),
    };
    let colors = PrysmProcessor::new(&Config::default()).into_stream(frames);
    tracing::info!(address = %config.wled_address, ?led_counts, "Streaming to WLED");

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
    use std::net::UdpSocket;
    use std::time::Duration;

    const TEST_CONFIG: &str = r#"
wled_address = "wled.local:4048"
[leds]
top = 96
right = 54
bottom = 96
left = 54
[source]
type = "video"
"#;

    fn parse(contents: &str) -> Result<RuntimeConfig> {
        let config: RuntimeConfig = toml::from_str(contents)?;
        config.led_counts()?;
        Ok(config)
    }

    #[test]
    fn selects_default_or_custom_config_and_shows_help() {
        assert_eq!(
            Args::try_parse_from(["led"]).unwrap().config,
            PathBuf::from("led.toml")
        );
        assert_eq!(
            Args::try_parse_from(["led", "--config", "/tmp/my LEDs.toml"])
                .unwrap()
                .config,
            PathBuf::from("/tmp/my LEDs.toml")
        );
        assert!(Args::try_parse_from(["led", "--unknown"]).is_err());
        for flag in ["--help", "-h"] {
            assert_eq!(
                Args::try_parse_from(["led", flag]).unwrap_err().kind(),
                clap::error::ErrorKind::DisplayHelp
            );
        }
    }

    #[test]
    fn parses_shipped_video_config_and_camera_source() {
        parse(include_str!("../../../led.toml")).unwrap();
        let contents = TEST_CONFIG;
        let config = parse(contents).unwrap();
        assert_eq!(config.wled_address, "wled.local:4048");
        assert_eq!(config.led_counts().unwrap(), [96, 54, 96, 54]);
        assert!(
            matches!(config.source, Source::Video { path } if path == Path::new(DEFAULT_VIDEO))
        );

        let config = parse(&contents.replace(
            "type = \"video\"",
            "type = \"camera\"\ndevice = \"/dev/video2\"",
        ))
        .unwrap();
        assert!(
            matches!(config.source, Source::Camera { device: Some(device) } if device == "/dev/video2")
        );
        let config = parse(&contents.replace("type = \"video\"", "type = \"camera\"")).unwrap();
        assert!(matches!(config.source, Source::Camera { device: None }));
        let config = parse(&contents.replace(
            "type = \"video\"",
            "type = \"video\"\npath = \"/tmp/my video.mp4\"",
        ))
        .unwrap();
        assert!(
            matches!(config.source, Source::Video { path } if path == Path::new("/tmp/my video.mp4"))
        );
    }

    #[test]
    fn rejects_invalid_configurations_and_counts() {
        let contents = TEST_CONFIG;
        for invalid in [
            String::new(),
            contents.replace("top = 96", "top = -1"),
            contents.replace("top = 96", "top = 65536"),
            contents.replace("top = 96", "top = 65535"),
            contents.replace("top = 96", "top = 'red'"),
            contents.replace("top = 96", "tpo = 96"),
            contents
                .replace("top = 96", "top = 0")
                .replace("right = 54", "right = 0")
                .replace("bottom = 96", "bottom = 0")
                .replace("left = 54", "left = 0"),
            contents.replace("type = \"video\"", "type = \"unknown\""),
            contents.replace(
                "type = \"video\"",
                "type = \"video\"\ndevice = \"/dev/video2\"",
            ),
            contents.replace("type = \"video\"", "type = \"camera\"\npath = \"clip.mp4\""),
            contents.replace("wled_address =", "unknown ="),
        ] {
            assert!(parse(&invalid).is_err(), "accepted {invalid:?}");
        }
    }

    #[tokio::test]
    async fn processes_a_frame_sends_colors_and_completes_at_eof() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut renderer = WledRenderer::new(socket.local_addr().unwrap()).unwrap();
        let frames = futures::stream::iter([Ok(Frame::fill(255, 2, 2, PixelFormat::RGB24))]);
        let config = Config {
            brightness_percent: 100,
            smoothing_seconds: 0.0,
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
