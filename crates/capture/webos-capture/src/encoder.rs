//! Encode complete frames without owning sockets or capture resources.
use anyhow::Result;
use prysm_metrics::{BytesKind, Codec, Content, EncoderEvent, Stage, WireBoundary};
use prysm_webos_protocol::{Compression, Header, compress, h264};

#[cfg(feature = "hardware-h264")]
mod hardware;

#[derive(Debug)]
pub struct HardwareFailed;
impl std::fmt::Display for HardwareFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("hardware encoder failed; reconnect with software")
    }
}
impl std::error::Error for HardwareFailed {}

pub enum H264 {
    Software(Box<h264::Encoder>),
    #[cfg(feature = "hardware-h264")]
    Pending {
        fps: u8,
        bitrate: u32,
    },
    #[cfg(feature = "hardware-h264")]
    Hardware {
        encoder: Box<hardware::Hardware>,
        emitted: bool,
        fps: u8,
        bitrate: u32,
    },
}
impl H264 {
    fn hardware_active(&self) -> bool {
        !matches!(self, Self::Software(_))
    }
    fn encode(
        &mut self,
        header: Header,
        pixels: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<Option<h264::FrameKind>> {
        #[cfg(feature = "hardware-h264")]
        if let Self::Pending { fps, bitrate } = *self {
            *self = match hardware::Hardware::new(header, fps, bitrate) {
                Ok(encoder) => Self::Hardware {
                    encoder: Box::new(encoder),
                    emitted: false,
                    fps,
                    bitrate,
                },
                Err(error) => {
                    tracing::warn!(%error, "hardware unavailable; using OpenH264");
                    Self::Software(Box::new(h264::Encoder::new(bitrate, fps)?))
                }
            };
        }
        match self {
            Self::Software(encoder) => encoder.encode(header, pixels, output),
            #[cfg(feature = "hardware-h264")]
            Self::Hardware {
                encoder,
                emitted,
                fps,
                bitrate,
            } => match encoder.encode(header, pixels, output) {
                Ok(kind) => {
                    *emitted |= kind.is_some();
                    Ok(kind)
                }
                Err(error) if *emitted => Err(error.context(HardwareFailed)),
                Err(error) => {
                    tracing::warn!(%error, "hardware first frame failed; using OpenH264");
                    let software = h264::Encoder::new(*bitrate, *fps)?;
                    *self = Self::Software(Box::new(software));
                    self.encode(header, pixels, output)
                }
            },
            #[cfg(feature = "hardware-h264")]
            Self::Pending { .. } => unreachable!("initialized above"),
        }
    }
}

pub const H264_BITRATE: u32 = 2_000_000;

#[derive(Clone, Copy)]
pub struct EncodedFrame<'a> {
    pub header: Header,
    pub pixels: &'a [u8],
    pub content: Content,
}

pub enum Encoder {
    Raw,
    Lz4(Vec<u8>),
    H264 {
        encoder: H264,
        output: Vec<u8>,
        pending: Option<(Header, Content)>,
    },
}

impl Encoder {
    pub fn new(compression: Compression, fps: u8, bitrate: u32) -> Result<Self> {
        Ok(match compression {
            Compression::None => Self::Raw,
            Compression::Lz4 => Self::Lz4(Vec::new()),
            Compression::H264Software | Compression::H264Hardware => Self::H264 {
                encoder: if compression == Compression::H264Hardware {
                    #[cfg(not(feature = "hardware-h264"))]
                    anyhow::bail!("hardware H.264 requires a build with the hardware-h264 feature");
                    #[cfg(feature = "hardware-h264")]
                    {
                        anyhow::ensure!(
                            (1..=60).contains(&fps) && (64_000..=100_000_000).contains(&bitrate),
                            "invalid H.264 rate configuration"
                        );
                        H264::Pending { fps, bitrate }
                    }
                } else {
                    H264::Software(Box::new(h264::Encoder::new(bitrate, fps).inspect_err(
                        |_| {
                            prysm_metrics::event(EncoderEvent::InitializationFailed);
                        },
                    )?))
                },
                output: Vec::new(),
                pending: None,
            },
        })
    }

    pub fn hardware_active(&self) -> bool {
        matches!(self, Self::H264 { encoder, .. } if encoder.hardware_active())
    }

    pub fn encode<'a>(
        &'a mut self,
        mut header: Header,
        pixels: &'a [u8],
        mut content: Content,
    ) -> Result<Option<EncodedFrame<'a>>> {
        let mut raw_len = header.raw_len().inspect_err(|_| {
            prysm_metrics::event(EncoderEvent::InputRejected);
        })?;
        if raw_len != pixels.len() {
            prysm_metrics::event(EncoderEvent::InputRejected);
            anyhow::bail!("incorrect sender frame length");
        }
        header.codec = prysm_webos_protocol::Codec::Raw;
        let encode_time = prysm_metrics::timer(Stage::FrameEncode);
        let payload = match self {
            Self::Raw => pixels,
            Self::Lz4(output) => {
                if let Some(length) = compress(pixels, output).inspect_err(|_| {
                    prysm_metrics::event(EncoderEvent::CompressionFailed);
                })? {
                    header.codec = prysm_webos_protocol::Codec::Lz4;
                    &output[..length]
                } else {
                    prysm_metrics::event(EncoderEvent::RawFallback);
                    pixels
                }
            }
            Self::H264 {
                encoder,
                output,
                pending,
            } => {
                let previous = pending.replace((header, content));
                let Some(kind) = encoder.encode(header, pixels, output).inspect_err(|_| {
                    prysm_metrics::event(EncoderEvent::H264EncodeFailed);
                })?
                else {
                    if !encoder.hardware_active() {
                        *pending = None;
                        prysm_metrics::event(EncoderEvent::H264RateControlSkipped);
                    }
                    return Ok(None);
                };
                if encoder.hardware_active() {
                    (header, content) = previous
                        .ok_or_else(|| anyhow::anyhow!("missing hardware input metadata"))?;
                    raw_len = header.raw_len()?;
                } else {
                    *pending = None;
                }
                prysm_metrics::event(match kind {
                    h264::FrameKind::Idr => EncoderEvent::H264IdrEncoded,
                    h264::FrameKind::Intra => EncoderEvent::H264IntraFrameEncoded,
                    h264::FrameKind::Predicted => EncoderEvent::H264PredictedFrameEncoded,
                });
                header.codec = prysm_webos_protocol::Codec::H264;
                header.format = prysm_webos_protocol::PixelFormat::Rgb;
                output.as_slice()
            }
        };
        drop(encode_time);
        header.payload_len = payload.len();
        prysm_metrics::bytes(BytesKind::EncoderInput, raw_len);
        prysm_metrics::bytes(BytesKind::EncoderOutput, payload.len());
        prysm_metrics::wire_frame(
            WireBoundary::Encoded,
            header.format.into(),
            Codec::from(&header),
            content,
            header.width,
            header.height,
        );
        Ok(Some(EncodedFrame {
            header,
            pixels: payload,
            content,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn h264_choices_select_available_backends_without_opening_hardware() {
        assert!(matches!(
            Encoder::new(Compression::H264Software, 15, H264_BITRATE).unwrap(),
            Encoder::H264 {
                encoder: H264::Software(_),
                ..
            }
        ));
        #[cfg(feature = "hardware-h264")]
        assert!(matches!(
            Encoder::new(Compression::H264Hardware, 15, H264_BITRATE).unwrap(),
            Encoder::H264 {
                encoder: H264::Pending { .. },
                ..
            }
        ));
        for compression in [Compression::H264Software, Compression::H264Hardware] {
            assert!(Encoder::new(compression, 0, H264_BITRATE).is_err());
            assert!(Encoder::new(compression, 15, 0).is_err());
        }
    }
    #[cfg(not(feature = "hardware-h264"))]
    #[test]
    fn hardware_selection_requires_the_build_feature() {
        let error = Encoder::new(Compression::H264Hardware, 15, H264_BITRATE)
            .err()
            .expect("hardware selection must fail without the feature");
        assert!(error.to_string().contains("hardware-h264 feature"));
    }
    #[cfg(all(feature = "hardware-h264", not(target_os = "linux")))]
    #[test]
    fn unavailable_hardware_falls_back_before_sending_any_reference() {
        let mut encoder = Encoder::new(Compression::H264Hardware, 15, H264_BITRATE).unwrap();
        let header = Header {
            width: 32,
            height: 34,
            format: prysm_webos_protocol::PixelFormat::Rgb,
            codec: prysm_webos_protocol::Codec::Raw,
            payload_len: 0,
        };
        let pixels = [255, 0, 0].repeat(32 * 34);
        let packet = encoder
            .encode(header, &pixels, Content::Video)
            .unwrap()
            .unwrap();
        h264::Decoder::new()
            .unwrap()
            .decode(packet.header, packet.pixels)
            .unwrap();
        assert!(!encoder.hardware_active());
    }
}
