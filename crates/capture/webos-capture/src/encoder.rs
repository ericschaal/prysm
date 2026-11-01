//! Encode complete frames without owning sockets or capture resources.
use anyhow::Result;
use prysm_metrics::{BytesKind, Codec, Content, EncoderEvent, Stage, WireBoundary};
use prysm_webos_protocol::{Compression, Header, compress, h264};

pub const H264_BITRATE: u32 = 2_000_000;

#[derive(Clone, Copy)]
pub struct EncodedFrame<'a> {
    pub header: Header,
    pub pixels: &'a [u8],
}

pub enum Encoder {
    Raw,
    Lz4(Vec<u8>),
    H264 {
        encoder: Box<h264::Encoder>,
        output: Vec<u8>,
    },
}

impl Encoder {
    pub fn new(compression: Compression, fps: u8, bitrate: u32) -> Result<Self> {
        Ok(match compression {
            Compression::None => Self::Raw,
            Compression::Lz4 => Self::Lz4(Vec::new()),
            Compression::H264 => Self::H264 {
                encoder: Box::new(h264::Encoder::new(bitrate, fps).inspect_err(|_| {
                    prysm_metrics::event(EncoderEvent::InitializationFailed);
                })?),
                output: Vec::new(),
            },
        })
    }

    pub fn encode<'a>(
        &'a mut self,
        mut header: Header,
        pixels: &'a [u8],
        content: Content,
    ) -> Result<Option<EncodedFrame<'a>>> {
        let raw_len = header.raw_len().inspect_err(|_| {
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
            Self::H264 { encoder, output } => {
                let Some(kind) = encoder.encode(header, pixels, output).inspect_err(|_| {
                    prysm_metrics::event(EncoderEvent::H264EncodeFailed);
                })?
                else {
                    prysm_metrics::event(EncoderEvent::H264RateControlSkipped);
                    return Ok(None);
                };
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
        prysm_metrics::bytes(BytesKind::EncoderInput, pixels.len());
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
        }))
    }
}
