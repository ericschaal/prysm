use clap::{Args, Parser, ValueEnum};
use std::net::SocketAddr;

/// Capture LG webOS video and UI and stream frames to Prysm.
#[derive(Debug, Parser)]
#[command(version)]
pub struct Cli {
    /// Prysm receiver address, including its port
    #[arg(value_name = "IP:PORT", value_parser = receiver)]
    pub receiver: SocketAddr,

    /// Opt-in Prometheus endpoint on a trusted LAN interface, e.g. 0.0.0.0:9101
    #[arg(long, value_name = "IP:PORT")]
    pub metrics_listen: Option<SocketAddr>,

    /// Layers to capture; video avoids color conversion on the TV
    #[arg(long, value_enum, default_value_t = Source::Both)]
    pub source: Source,

    /// Lossless wire compression; incompressible frames are sent raw
    #[arg(long, value_enum, default_value_t = Compression::None)]
    pub compression: Compression,

    /// Output format after UI composition; native video-only frames pass through
    #[arg(long, value_enum, default_value_t = CompositionFormat::Rgb)]
    pub composition_format: CompositionFormat,

    #[command(flatten)]
    pub capture: CaptureOptions,
}

#[derive(Debug, Args)]
pub struct CaptureOptions {
    /// Capture width (even, 2–1920)
    #[arg(long, default_value_t = 320, value_parser = dimension::<1920>)]
    pub width: u32,

    /// Capture height (even, 2–1080)
    #[arg(long, default_value_t = 180, value_parser = dimension::<1080>)]
    pub height: u32,

    /// Maximum frames per second
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u8).range(1..=60))]
    pub fps: u8,

    #[arg(long, value_enum, default_value_t = VideoBackend::Auto)]
    pub video_backend: VideoBackend,

    #[arg(long, value_enum, default_value_t = UiBackend::Auto)]
    pub ui_backend: UiBackend,

    /// Video pipeline capture point; support depends on the TV and backend
    #[arg(long, value_enum, default_value_t = CapturePoint::Display)]
    pub capture_point: CapturePoint,

    /// Use the optional /dev/forcecapture kernel module when vtcapture is not ready
    #[arg(long, help_heading = "Hardware settings")]
    pub force_capture: bool,

    /// Use `DILE_VT_CreateEx` on TVs that require it
    #[arg(long, help_heading = "Hardware settings")]
    pub dile_create_ex: bool,

    /// Skip freezing DILE buffers while copying (may produce tearing)
    #[arg(long, help_heading = "Hardware settings")]
    pub no_freeze: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Source {
    Video,
    Ui,
    Both,
}
impl Source {
    pub fn includes_video(self) -> bool {
        matches!(self, Self::Video | Self::Both)
    }
    pub fn includes_ui(self) -> bool {
        matches!(self, Self::Ui | Self::Both)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Compression {
    None,
    Lz4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum CompositionFormat {
    Rgb,
    Nv12,
}

impl From<CompositionFormat> for prysm_webos_protocol::PixelFormat {
    fn from(format: CompositionFormat) -> Self {
        match format {
            CompositionFormat::Rgb => Self::Rgb,
            CompositionFormat::Nv12 => Self::Nv12,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum VideoBackend {
    Auto,
    Vtcapture,
    Dile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum UiBackend {
    Auto,
    Gm,
    Halgal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[repr(i32)]
pub enum CapturePoint {
    ScalerInput = 0,
    ScalerOutput = 1,
    Display = 2,
    Blended = 3,
    Osd = 4,
    Histogram = 5,
}

fn dimension<const MAX: u32>(value: &str) -> Result<u32, String> {
    let dimension = value.parse::<u32>().map_err(|error| error.to_string())?;
    if (2..=MAX).contains(&dimension) && dimension.is_multiple_of(2) {
        Ok(dimension)
    } else {
        Err(format!("must be an even number from 2 to {MAX}"))
    }
}

fn receiver(value: &str) -> Result<SocketAddr, String> {
    let address = value
        .parse::<SocketAddr>()
        .map_err(|error| error.to_string())?;
    if address.port() == 0 {
        Err("receiver port must be nonzero".into())
    } else {
        Ok(address)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, error::ErrorKind};

    fn parse(args: &str) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("capture").chain(args.split_whitespace()))
    }

    #[test]
    fn typed_options_and_layer_selection() {
        Cli::command().debug_assert();
        let cli = parse("[::1]:19400 --source ui --ui-backend halgal --compression lz4").unwrap();
        assert_eq!(cli.compression, Compression::Lz4);
        assert!(!cli.source.includes_video());
        assert!(cli.source.includes_ui());
        assert_eq!(cli.capture.ui_backend, UiBackend::Halgal);
        assert_eq!(cli.composition_format, CompositionFormat::Rgb);
        assert_eq!(
            parse("127.0.0.1:19400 --composition-format nv12")
                .unwrap()
                .composition_format,
            CompositionFormat::Nv12
        );
        assert_eq!(
            parse("127.0.0.1:19400 --capture-point osd --force-capture")
                .unwrap()
                .capture
                .capture_point,
            CapturePoint::Osd
        );
        assert_eq!(parse("--help").unwrap_err().kind(), ErrorKind::DisplayHelp);
        assert_eq!(
            parse("--version").unwrap_err().kind(),
            ErrorKind::DisplayVersion
        );
    }

    #[test]
    fn invalid_inputs_fail_before_native_initialization() {
        for args in [
            "",
            "127.0.0.1",
            "127.0.0.1:0",
            "127.0.0.1:19400 --width 3",
            "127.0.0.1:19400 --height 1082",
            "127.0.0.1:19400 --fps 0",
            "127.0.0.1:19400 --fps 61",
            "127.0.0.1:19400 --video-backend unknown",
            "127.0.0.1:19400 --quirks 4",
            "127.0.0.1:19400 --ui-backend none",
            "127.0.0.1:19400 --composition-format nv16",
        ] {
            assert!(parse(args).is_err(), "{args}");
        }
    }
}

impl From<Source> for prysm_metrics::CaptureSource {
    fn from(source: Source) -> Self {
        match source {
            Source::Video => Self::Video,
            Source::Ui => Self::Ui,
            Source::Both => Self::Both,
        }
    }
}
impl From<Compression> for prysm_metrics::Codec {
    fn from(codec: Compression) -> Self {
        match codec {
            Compression::None => Self::None,
            Compression::Lz4 => Self::Lz4,
        }
    }
}
