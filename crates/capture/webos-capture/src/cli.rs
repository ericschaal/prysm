use clap::{Args, Parser, ValueEnum};
use prysm_webos_protocol::ReceiverAddress;
pub use prysm_webos_protocol::{Compression, Source};
use std::net::SocketAddr;

/// Capture LG webOS video and UI and stream frames to Prysm.
#[derive(Debug, Parser)]
#[command(version)]
pub struct Cli {
    /// Prysm receiver address, including its port
    #[arg(value_name = "IP:PORT")]
    pub receiver: ReceiverAddress,

    /// Opt-in Prometheus endpoint on a trusted LAN interface, e.g. 0.0.0.0:9101
    #[arg(long, value_name = "IP:PORT")]
    pub metrics_listen: Option<SocketAddr>,

    /// Layers to capture; video avoids color conversion on the TV
    #[arg(long, value_enum, default_value_t = Source::Both)]
    pub source: Source,

    /// Wire codec; H.264 is lossy and uses a persistent low-latency encoder
    #[arg(long, value_enum, default_value_t = Compression::None)]
    pub compression: Compression,

    /// Output format after UI composition; native video-only frames pass through
    #[arg(long, value_enum, default_value_t = CompositionFormat::Rgb)]
    pub composition_format: CompositionFormat,

    /// H.264 target bitrate in bits per second
    #[arg(long, default_value_t = 2_000_000, value_parser = clap::value_parser!(u32).range(64_000..=100_000_000))]
    pub h264_bitrate: u32,

    #[command(flatten)]
    pub capture: CaptureOptions,
}

#[derive(Debug, Clone, Args)]
pub struct CaptureOptions {
    /// Negotiated with Prysm before capture starts
    #[arg(skip = 320u32)]
    pub width: u32,

    /// Negotiated with Prysm before capture starts
    #[arg(skip = 180u32)]
    pub height: u32,

    /// Negotiated with Prysm before capture starts
    #[arg(skip = 30)]
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
pub enum CompositionFormat {
    Rgb,
    Nv12,
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
        let h264 = parse("127.0.0.1:19400 --compression h264 --h264-bitrate 1500000").unwrap();
        assert_eq!(h264.compression, Compression::H264);
        assert_eq!(h264.h264_bitrate, 1_500_000);
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
            "127.0.0.1:19400 --exit-on-disconnect",
            "127.0.0.1:19400 --report-connection",
            "127.0.0.1:19400 --width 3",
            "127.0.0.1:19400 --height 1082",
            "127.0.0.1:19400 --fps 30",
            "127.0.0.1:19400 --h264-bitrate 0",
            "127.0.0.1:19400 --video-backend unknown",
            "127.0.0.1:19400 --quirks 4",
            "127.0.0.1:19400 --ui-backend none",
            "127.0.0.1:19400 --composition-format nv16",
        ] {
            assert!(parse(args).is_err(), "{args}");
        }
    }
}
