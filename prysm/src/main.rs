use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use futures::{Stream, StreamExt, stream::BoxStream};
use led_renderer::WledRenderer;
use prysm::video::video_feed;
use prysm_capture::{Capturer, Frame, PrysmCapturer};
use prysm_core::{CaptureResolution, EdgeColors};
use prysm_processor::PrysmProcessor;
use tokio_util::sync::CancellationToken;

mod config;
use config::{Input, Output, RuntimeConfig};

#[derive(Debug, Parser)]
#[command(about = "Process video from a device or file for WLED or an on-screen preview")]
struct Args {
    /// Runtime TOML configuration file
    #[arg(long, default_value = "prysm.toml", value_name = "PATH")]
    config: PathBuf,
}

impl Input {
    fn into_stream(
        self,
        resolution: CaptureResolution,
        shutdown: CancellationToken,
    ) -> Result<BoxStream<'static, Result<Frame>>> {
        let [width, height] = resolution.dimensions();
        match self {
            Self::Device { device } => Ok(Capturer::new(Some(device.as_str()), shutdown)?
                .into_stream(width, height)
                .map(|frame| frame.map_err(anyhow::Error::from))
                .boxed()),
            Self::File { path } => Ok(video_feed(path.as_path(), resolution, shutdown)?.boxed()),
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config = RuntimeConfig::load(&args.config)?;
    tracing::subscriber::set_global_default(tracing_subscriber::FmtSubscriber::new())?;
    let settings = config.settings;
    let resolution = settings.capture_resolution;
    match config.output {
        Output::Egui { refresh_rate_hz } => {
            prysm::run(settings, refresh_rate_hz, move |shutdown| {
                config.input.into_stream(resolution, shutdown)
            })
        }
        Output::Wled {
            address,
            refresh_rate_hz,
            leds,
        } => {
            let counts = leds.counts();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("Failed to build tokio runtime")?;
            runtime.block_on(async move {
                let mut renderer = WledRenderer::new(address.as_str()).with_context(|| {
                    format!("Failed to connect to WLED at {}", address.as_str())
                })?;
                renderer.set_refresh_rate_hz(refresh_rate_hz);
                let shutdown = CancellationToken::new();
                let _shutdown_guard = shutdown.clone().drop_guard();
                let frames = config.input.into_stream(resolution, shutdown.clone())?;
                let colors = PrysmProcessor::new(&settings).into_stream(frames);
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
    use prysm_capture::PixelFormat;
    use prysm_core::{BrightnessPercent, Config, SampleDensity, SmoothingSeconds};
    use std::net::UdpSocket;
    use std::path::Path;
    use std::time::Duration;

    const TEST_CONFIG: &str = r#"
[input]
type = "file"
path = "/tmp/my video.mp4"
[output]
type = "wled"
address = "wled.local:4048"
[output.leds]
top = 96
right = 54
bottom = 96
left = 54
"#;

    fn parse(contents: &str) -> Result<RuntimeConfig> {
        let config: RuntimeConfig = toml::from_str(contents)?;
        Ok(config)
    }

    #[test]
    fn selects_default_or_custom_config_and_shows_help() {
        assert_eq!(
            Args::try_parse_from(["prysm"]).unwrap().config,
            PathBuf::from("prysm.toml")
        );
        assert_eq!(
            Args::try_parse_from(["prysm", "--config", "/tmp/my config.toml"])
                .unwrap()
                .config,
            PathBuf::from("/tmp/my config.toml")
        );
        assert!(Args::try_parse_from(["prysm", "--unknown"]).is_err());
        for flag in ["--help", "-h"] {
            assert_eq!(
                Args::try_parse_from(["prysm", flag]).unwrap_err().kind(),
                clap::error::ErrorKind::DisplayHelp
            );
        }
    }

    #[test]
    fn parses_all_input_output_combinations_and_shipped_configs() {
        let wled_output = &TEST_CONFIG[TEST_CONFIG.find("[output]").unwrap()..];
        for input in [
            "[input]\ntype = 'device'\ndevice = '/dev/video2'\n",
            "[input]\ntype = 'device'\ndevice = '0x2200000345f2131'\n",
            "[input]\ntype = 'file'\npath = '/tmp/my video.mp4'\n",
        ] {
            for output in ["[output]\ntype = 'egui'\n", wled_output] {
                let config = parse(&format!("{input}{output}")).unwrap();
                assert_eq!(config.settings, Config::default());
            }
        }
        let config = parse(TEST_CONFIG).unwrap();
        assert!(
            matches!(config.input, Input::File { path } if path.as_path() == Path::new("/tmp/my video.mp4"))
        );
        assert!(
            matches!(config.output, Output::Wled { address, refresh_rate_hz, leds } if address.as_str() == "wled.local:4048" && refresh_rate_hz.get() == 30 && leds.counts() == [96, 54, 96, 54])
        );
        parse(include_str!("../../prysm.toml")).unwrap();
        parse(include_str!("../debian/prysm.toml")).unwrap();
    }

    #[test]
    fn parses_custom_wled_refresh_rate_and_rejects_invalid_rates() {
        let config =
            parse(&TEST_CONFIG.replace("[output.leds]", "refresh_rate_hz = 60\n[output.leds]"))
                .unwrap();
        assert!(
            matches!(config.output, Output::Wled { refresh_rate_hz, .. } if refresh_rate_hz.get() == 60)
        );
        for value in ["0", "-1", "1.5", "'fast'", "4294967296"] {
            let contents = TEST_CONFIG.replace(
                "[output.leds]",
                &format!("refresh_rate_hz = {value}\n[output.leds]"),
            );
            assert!(
                parse(&contents).is_err(),
                "accepted refresh_rate_hz = {value}"
            );
        }
    }

    #[test]
    fn parses_egui_refresh_rate_and_rejects_invalid_rates() {
        let contents =
            "[input]\ntype = 'device'\ndevice = '/dev/video0'\n[output]\ntype = 'egui'\n";
        let config = parse(contents).unwrap();
        assert!(
            matches!(config.output, Output::Egui { refresh_rate_hz } if refresh_rate_hz.get() == 30)
        );
        let config = parse(&format!("{contents}refresh_rate_hz = 60")).unwrap();
        assert!(
            matches!(config.output, Output::Egui { refresh_rate_hz } if refresh_rate_hz.get() == 60)
        );
        for value in ["0", "-1", "1.5", "'fast'", "4294967296"] {
            assert!(
                parse(&format!("{contents}refresh_rate_hz = {value}")).is_err(),
                "accepted refresh_rate_hz = {value}"
            );
        }
    }

    #[test]
    fn applies_custom_processor_settings_and_capture_resolution() {
        let config = parse(&format!("brightness_percent = 60\nsample_density = 200\ncapture_resolution = [1280, 720]\n{TEST_CONFIG}")).unwrap();
        assert_eq!(config.settings.capture_resolution.dimensions(), [1280, 720]);
        let settings = config.settings;
        assert_eq!(settings.brightness_percent, BrightnessPercent(60));
        assert_eq!(settings.sample_density, SampleDensity(200));
    }

    #[test]
    fn rejects_invalid_configurations_and_counts() {
        let contents = TEST_CONFIG;
        for invalid in [
            String::new(),
            format!("sample_density = 0\n{contents}"),
            format!("sample_density = -1\n{contents}"),
            format!("capture_resolution = [0, 360]\n{contents}"),
            format!("capture_resolution = [640, 0]\n{contents}"),
            format!("capture_resolution = [4294967295, 4294967295]\n{contents}"),
            format!("capture_resolution = [-1, 360]\n{contents}"),
            format!("capture_resolution = [640]\n{contents}"),
            format!("capture_resolution = [640, 360, 1]\n{contents}"),
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
            contents.replace("type = \"file\"", "type = \"unknown\""),
            contents.replace("type = \"file\"", "type = \"camera\""),
            contents.replace("type = \"file\"", "type = \"device\""),
            contents.replace("path = \"/tmp/my video.mp4\"", ""),
            contents.replace("path = \"/tmp/my video.mp4\"", "path = ''"),
            contents.replace("path = \"/tmp/my video.mp4\"", "device = '/dev/video0'"),
            contents.replace("type = \"wled\"", "type = \"egui\""),
            contents.replace("type = \"wled\"", "type = \"unknown\""),
            contents.replace("address =", "unknown ="),
            contents.replace("address = \"wled.local:4048\"", "address = ''"),
            format!("brightness_percent = -1\n{contents}"),
            format!("brightness_percent = 256\n{contents}"),
            format!("brightness_percent = 'bright'\n{contents}"),
            "[input]\ntype = 'device'\n[output]\ntype = 'egui'".to_owned(),
            "[input]\ntype = 'device'\ndevice = ''\n[output]\ntype = 'egui'".to_owned(),
        ] {
            assert!(parse(&invalid).is_err(), "accepted {invalid:?}");
        }
    }

    #[test]
    fn file_input_preserves_startup_failure_context() {
        let input = Input::File {
            path: PathBuf::from("/missing/prysm-input.mp4")
                .try_into()
                .unwrap(),
        };
        let error = input
            .into_stream(CaptureResolution::default(), CancellationToken::new())
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "Cannot open video /missing/prysm-input.mp4"
        );
        assert!(error.downcast_ref::<std::io::Error>().is_some());
    }

    #[tokio::test]
    async fn processes_a_frame_sends_colors_and_completes_at_eof() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut renderer = WledRenderer::new(socket.local_addr().unwrap()).unwrap();
        let frames = futures::stream::iter([Ok(Frame::fill(255, 2, 2, PixelFormat::RGB24))]);
        let runtime_config = parse(&format!("brightness_percent = 100\n{TEST_CONFIG}")).unwrap();
        let config = Config {
            smoothing_seconds: SmoothingSeconds(0.0),
            ..runtime_config.settings
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
