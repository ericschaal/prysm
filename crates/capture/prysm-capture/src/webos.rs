//! Receive raw/LZ4 pixels or connection-scoped H.264 from the Rust webOS sender.
use crate::receiver::Capture;
use crate::{Frame, FramePool, PixelFormat};
use anyhow::Context;
use prysm_metrics::{BytesKind, Codec, DecoderEvent, FrameReceiverEvent, Stage, WireBoundary};
use prysm_webos_protocol::{HEADER_LEN, Header, PixelFormat as WireFormat};
use std::time::Duration;

pub(super) async fn receive_frames(socket: &mut axum::extract::ws::WebSocket, capture: &Capture) {
    let mut decoder = Decoder::default();
    let mut stats = prysm_metrics::TraceStats::new("webos_receive");
    tracing::info!("webOS receiving frames");
    loop {
        let started = stats.start();
        // A stalled or partial frame must not hold the only TV connection forever.
        match tokio::time::timeout(Duration::from_secs(5), async {
            let wait = prysm_metrics::timer(Stage::FrameReadMessage);
            let bytes = crate::receiver::binary(socket).await?;
            drop(wait);
            decoder.decode(&bytes)
        })
        .await
        {
            Ok(Ok(frame)) => {
                stats.record(started, frame.len(), Some(decoder.pool.stats()));
                if !capture.publish(frame) {
                    return;
                }
            }
            Ok(Err(error)) => {
                stats.failure();
                tracing::info!( error = %format!("{error:#}"), "webOS connection ended");
                break;
            }
            Err(_) => {
                stats.failure();
                prysm_metrics::event(FrameReceiverEvent::FrameTimedOut);
                tracing::warn!("webOS frame timed out");
                break;
            }
        }
    }
}

#[derive(Default)]
struct Decoder {
    h264: Option<prysm_webos_protocol::h264::Decoder>,
    pool: FramePool,
    decompressed: Vec<u8>,
}

impl Decoder {
    fn decode(&mut self, bytes: &[u8]) -> anyhow::Result<Frame> {
        let header = Header::decode(
            bytes
                .get(..HEADER_LEN)
                .context("truncated capture header")?
                .try_into()?,
        )
        .inspect_err(|_| prysm_metrics::event(FrameReceiverEvent::HeaderRejected))?;
        let payload = &bytes[HEADER_LEN..];
        if payload.len() != header.payload_len {
            prysm_metrics::event(FrameReceiverEvent::PayloadTruncated);
            anyhow::bail!("capture payload length differs from header");
        }
        prysm_metrics::wire_frame(
            WireBoundary::PayloadReceived,
            header.format.into(),
            Codec::from(&header),
            prysm_metrics::Content::Unspecified,
            header.width,
            header.height,
        );
        prysm_metrics::bytes(BytesKind::ReceiverPayload, payload.len());
        let mut frame = self
            .pool
            .acquire(header.width, header.height, PixelFormat::RGB24)?;
        if header.codec == prysm_webos_protocol::Codec::H264 {
            let _time = prysm_metrics::timer(Stage::FrameDecodeH264);
            if self.h264.is_none() {
                self.h264 = Some(prysm_webos_protocol::h264::Decoder::new().inspect_err(|_| {
                    prysm_metrics::event(DecoderEvent::InitializationFailed);
                })?);
            }
            self.h264
                .as_mut()
                .expect("initialized decoder")
                .decode_into(header, payload, frame.as_mut_slice())
                .inspect_err(|_| prysm_metrics::event(DecoderEvent::H264DecodeFailed))?;
            return Ok(frame.finish());
        }
        let raw = if header.codec == prysm_webos_protocol::Codec::Lz4 {
            let _time = prysm_metrics::timer(Stage::FrameDecompress);
            self.decompressed.resize(header.raw_len()?, 0);
            prysm_webos_protocol::decompress(payload, &mut self.decompressed)
                .inspect_err(|_| prysm_metrics::event(DecoderEvent::DecompressionFailed))?;
            self.decompressed.as_slice()
        } else {
            payload
        };
        if header.format == WireFormat::Rgb {
            frame.as_mut_slice().copy_from_slice(raw);
            return Ok(frame.finish());
        }
        {
            let _time = prysm_metrics::timer(Stage::FrameConvert);
            let (y, uv) = raw.split_at(header.width as usize * header.height as usize);
            let image = yuv::YuvBiPlanarImage {
                y_plane: y,
                y_stride: header.width,
                uv_plane: uv,
                uv_stride: header.width,
                width: header.width,
                height: header.height,
            };
            let convert = match header.format {
                WireFormat::Nv12 => yuv::yuv_nv12_to_rgb,
                WireFormat::Nv16 => yuv::yuv_nv16_to_rgb,
                WireFormat::Rgb => unreachable!(),
            };
            convert(
                &image,
                frame.as_mut_slice(),
                header.width * 3,
                yuv::YuvRange::Limited,
                yuv::YuvStandardMatrix::Bt709,
                yuv::YuvConversionMode::Balanced,
            )
            .inspect_err(|_| prysm_metrics::event(DecoderEvent::ConversionFailed))?;
        }
        Ok(frame.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn read_frame(bytes: &[u8]) -> anyhow::Result<Frame> {
        Decoder::default().decode(bytes)
    }

    fn raw_rgb_packet(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
        let mut packet = prysm_webos_protocol::MAGIC.to_vec();
        for value in [width, height, 0, 0, u32::try_from(pixels.len()).unwrap()] {
            packet.extend_from_slice(&value.to_be_bytes());
        }
        packet.extend_from_slice(pixels);
        packet
    }

    fn fixture_packet(format: WireFormat, compressed: bool) -> Vec<u8> {
        let raw = match format {
            WireFormat::Rgb => vec![42; 24],
            WireFormat::Nv12 => [vec![16; 8], vec![128; 4]].concat(),
            WireFormat::Nv16 => [vec![16; 8], vec![128; 8]].concat(),
        };
        let mut scratch = Vec::new();
        let payload = if compressed {
            let length = prysm_webos_protocol::compress(&raw, &mut scratch)
                .unwrap()
                .unwrap();
            &scratch[..length]
        } else {
            &raw
        };
        let mut packet = Header {
            width: 4,
            height: 2,
            format,
            codec: if compressed {
                prysm_webos_protocol::Codec::Lz4
            } else {
                prysm_webos_protocol::Codec::Raw
            },
            payload_len: payload.len(),
        }
        .encode()
        .unwrap()
        .to_vec();
        packet.extend_from_slice(payload);
        packet
    }

    #[tokio::test]
    async fn telemetry_separates_header_payload_and_decoder_failures() {
        let exporter =
            prysm_metrics::start("127.0.0.1:0".parse().unwrap(), prysm_metrics::Role::Prysm)
                .unwrap();
        let mut malformed = fixture_packet(WireFormat::Rgb, true);
        malformed[HEADER_LEN] = 0xff;
        assert!(read_frame(&malformed).is_err());
        let mut rejected = fixture_packet(WireFormat::Rgb, false);
        rejected[0] = 0;
        assert!(read_frame(&rejected).is_err());
        let truncated = &fixture_packet(WireFormat::Rgb, false)[..=HEADER_LEN];
        assert!(read_frame(truncated).is_err());
        let address = exporter.address();
        let text = tokio::task::spawn_blocking(move || {
            use std::io::{Read, Write};
            let mut socket = std::net::TcpStream::connect(address).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            socket.write_all(b"GET /metrics HTTP/1.0\r\n\r\n").unwrap();
            let mut text = String::new();
            socket.read_to_string(&mut text).unwrap();
            text
        })
        .await
        .unwrap();
        for labels in [
            "component=\"FrameReceiver\",operation=\"ValidateHeader\",outcome=\"Rejected\"",
            "component=\"FrameReceiver\",operation=\"ReadPayload\",outcome=\"Truncated\"",
            "component=\"FrameDecoder\",operation=\"Decompress\",outcome=\"Failed\"",
            "component=\"FrameReceiver\",boundary=\"PayloadReceived\",format=\"Rgb24\",codec=\"Lz4\"",
        ] {
            assert!(text.contains(labels), "missing {labels}: {text}");
        }
    }

    #[tokio::test]
    async fn raw_lz4_rgb_and_chroma_keep_boundaries() {
        let mut decoder = Decoder::default();
        for (format, compressed, value) in [
            (WireFormat::Rgb, false, 42),
            (WireFormat::Rgb, true, 42),
            (WireFormat::Nv12, false, 0),
            (WireFormat::Nv16, false, 0),
        ] {
            let frame = decoder.decode(&fixture_packet(format, compressed)).unwrap();
            assert_eq!(frame.as_slice(), [value; 24]);
        }
    }

    #[tokio::test]
    async fn bt709_limited_native_colors_match_across_all_codecs() {
        use prysm_webos_protocol::{Codec, h264::Encoder};
        for format in [WireFormat::Nv12, WireFormat::Nv16] {
            for (y, u, v, expected) in [
                (16, 128, 128, [0, 0, 0]),
                (235, 128, 128, [255, 255, 255]),
                (63, 102, 240, [255, 0, 0]),
                (173, 42, 26, [0, 255, 0]),
                (32, 240, 118, [0, 0, 255]),
            ] {
                let input = Header {
                    width: 32,
                    height: 16,
                    format,
                    codec: Codec::Raw,
                    payload_len: 0,
                };
                let raw = [
                    vec![y; 512],
                    [u, v].repeat((input.raw_len().unwrap() - 512) / 2),
                ]
                .concat();
                for codec in [Codec::Raw, Codec::Lz4, Codec::H264] {
                    let mut header = Header { codec, ..input };
                    let mut payload = Vec::new();
                    match codec {
                        Codec::Raw => payload.extend_from_slice(&raw),
                        Codec::Lz4 => {
                            let len = prysm_webos_protocol::compress(&raw, &mut payload)
                                .unwrap()
                                .unwrap();
                            payload.truncate(len);
                        }
                        Codec::H264 => {
                            Encoder::new(2_000_000, 30)
                                .unwrap()
                                .encode(input, &raw, &mut payload)
                                .unwrap()
                                .unwrap();
                            header.format = WireFormat::Rgb;
                        }
                    }
                    header.payload_len = payload.len();
                    let mut packet = header.encode().unwrap().to_vec();
                    packet.extend_from_slice(&payload);
                    let frame = read_frame(&packet).unwrap();
                    assert!(
                        frame.as_slice().as_chunks::<3>().0.iter().all(|pixel| {
                            pixel
                                .iter()
                                .zip(expected)
                                .all(|(actual, expected)| actual.abs_diff(expected) <= 3)
                        }),
                        "{format:?}/{codec:?}: wrong color for {expected:?}"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn compressed_chroma_decodes_and_malformed_blocks_disconnect() {
        for format in [WireFormat::Nv12, WireFormat::Nv16] {
            let mut header = Header {
                width: 32,
                height: 16,
                format,
                codec: prysm_webos_protocol::Codec::Raw,
                payload_len: 0,
            };
            let raw = [vec![16; 512], vec![128; header.raw_len().unwrap() - 512]].concat();
            let mut scratch = Vec::new();
            let length = prysm_webos_protocol::compress(&raw, &mut scratch)
                .unwrap()
                .unwrap();
            header.codec = prysm_webos_protocol::Codec::Lz4;
            header.payload_len = length;
            let mut packet = header.encode().unwrap().to_vec();
            packet.extend_from_slice(&scratch[..length]);
            assert!(
                read_frame(&packet)
                    .unwrap()
                    .as_slice()
                    .iter()
                    .all(|v| *v <= 1)
            );
            packet[HEADER_LEN] = 0xff;
            assert!(read_frame(&packet).is_err());
        }
        let mut packet = fixture_packet(WireFormat::Rgb, false);
        for (offset, value) in [(4, u32::MAX), (12, 99), (16, 99), (20, u32::MAX)] {
            let mut bad = packet.clone();
            bad[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
            assert!(read_frame(&bad).is_err());
        }
        packet.pop();
        assert!(read_frame(&packet).is_err());
    }

    #[tokio::test]
    async fn rejects_bad_headers_and_truncated_pixels_without_panicking() {
        let mut bad_magic = raw_rgb_packet(1, 1, &[0; 3]);
        bad_magic[0] = 0;
        for bytes in [
            bad_magic,
            raw_rgb_packet(0, 1, &[]),
            raw_rgb_packet(u32::MAX, u32::MAX, &[]),
            raw_rgb_packet(1921, 1, &[]),
            raw_rgb_packet(1, 1081, &[]),
            raw_rgb_packet(2, 2, &[0; 3]),
            raw_rgb_packet(1, 1, &[0; 3])[..HEADER_LEN + 2].to_vec(),
        ] {
            assert!(read_frame(&bytes).is_err());
        }
    }

    #[tokio::test]
    async fn h264_packets_decode_in_order_with_reused_connection_state() {
        let mut encoder = prysm_webos_protocol::h264::Encoder::new(2_000_000, 30).unwrap();
        let mut decoder = Decoder::default();
        let mut pixels = None;
        let header = Header {
            width: 64,
            height: 32,
            format: WireFormat::Rgb,
            codec: prysm_webos_protocol::Codec::Raw,
            payload_len: 64 * 32 * 3,
        };
        for color in [[255, 0, 0], [0, 255, 0], [0, 0, 255]] {
            let mut payload = Vec::new();
            encoder
                .encode(header, &color.repeat(64 * 32), &mut payload)
                .unwrap();
            let wire = Header {
                codec: prysm_webos_protocol::Codec::H264,
                payload_len: payload.len(),
                ..header
            };
            let packet = [wire.encode().unwrap().as_slice(), &payload].concat();
            let frame = decoder.decode(&packet).unwrap();
            assert_eq!(frame.width(), 64);
            assert_eq!(frame.height(), 32);
            assert!(decoder.h264.is_some());
            assert_eq!(
                *pixels.get_or_insert(frame.as_slice().as_ptr()),
                frame.as_slice().as_ptr()
            );
            assert!(frame.as_slice().as_chunks::<3>().0.iter().all(|pixel| {
                pixel
                    .iter()
                    .zip(color)
                    .all(|(&actual, expected)| actual.abs_diff(expected) < 20)
            }));
        }
    }
}
