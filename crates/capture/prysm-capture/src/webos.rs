//! Receive raw/LZ4 pixels or connection-scoped H.264 from the Rust webOS sender.
use crate::{Frame, PixelFormat};
use anyhow::Context;
use prysm_metrics::{
    BytesKind, Codec, DecoderEvent, FrameReceiverEvent, FrameStage, Stage, WireBoundary,
};
use prysm_webos_protocol::{HEADER_LEN, Header, PixelFormat as WireFormat};
use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, sync::watch};

pub(super) async fn receive_frames(
    socket: &mut tokio::net::TcpStream,
    sender: &watch::Sender<Option<Frame>>,
    receiving: &watch::Sender<bool>,
    peer: SocketAddr,
    last_decoded: &mut Option<Instant>,
) {
    let mut scratch = [Vec::new(), Vec::new()];
    let mut decoder = None;
    loop {
        // A stalled or partial frame must not hold the only TV connection forever.
        match tokio::time::timeout(
            Duration::from_secs(5),
            read_frame_into(socket, &mut scratch, &mut decoder),
        )
        .await
        {
            Ok(Ok(frame)) => {
                if prysm_metrics::enabled() {
                    let now = Instant::now();
                    if let Some(last) = last_decoded.replace(now) {
                        prysm_metrics::observe(Stage::ReceiverFrameGap, now.duration_since(last));
                    }
                }
                prysm_metrics::frame(FrameStage::Decoded, frame.format().into());
                if sender.send(Some(frame)).is_err() {
                    return;
                }
                receiving.send_if_modified(|available| {
                    let changed = !*available;
                    *available = true;
                    changed
                });
            }
            Ok(Err(error)) => {
                tracing::debug!(%peer, error = %format!("{error:#}"), "webOS connection ended");
                break;
            }
            Err(_) => {
                prysm_metrics::event(FrameReceiverEvent::FrameTimedOut);
                tracing::warn!(%peer, "webOS frame timed out");
                break;
            }
        }
    }
}

#[cfg(test)]
async fn read_frame(reader: &mut (impl tokio::io::AsyncRead + Unpin)) -> anyhow::Result<Frame> {
    read_frame_into(reader, &mut [Vec::new(), Vec::new()], &mut None).await
}

async fn read_frame_into(
    reader: &mut (impl tokio::io::AsyncRead + Unpin),
    scratch: &mut [Vec<u8>; 2],
    decoder: &mut Option<prysm_webos_protocol::h264::Decoder>,
) -> anyhow::Result<Frame> {
    let mut header = [0; HEADER_LEN];
    {
        let _time = prysm_metrics::timer(Stage::FrameReadHeader);
        reader
            .read_exact(&mut header)
            .await
            .inspect_err(|error| {
                prysm_metrics::event(if error.kind() == std::io::ErrorKind::UnexpectedEof {
                    FrameReceiverEvent::HeaderClosed
                } else {
                    FrameReceiverEvent::HeaderReadFailed
                });
            })
            .context("reading webOS frame header")?;
    }
    let header = Header::decode(&header)
        .inspect_err(|_| prysm_metrics::event(FrameReceiverEvent::HeaderRejected))?;
    if header.codec == prysm_webos_protocol::Codec::H264 {
        scratch[0].resize(header.payload_len, 0);
        read_payload(reader, &mut scratch[0], header).await?;
        let _time = prysm_metrics::timer(Stage::FrameDecodeH264);
        if decoder.is_none() {
            *decoder = Some(
                prysm_webos_protocol::h264::Decoder::new()
                    .inspect_err(|_| prysm_metrics::event(DecoderEvent::InitializationFailed))?,
            );
        }
        let data = decoder
            .as_mut()
            .expect("initialized decoder")
            .decode(header, &scratch[0])
            .inspect_err(|_| prysm_metrics::event(DecoderEvent::H264DecodeFailed))?;
        return Ok(Frame::new(
            data,
            header.width,
            header.height,
            PixelFormat::RGB24,
        )?);
    }
    read_pixels(reader, header, scratch).await
}

async fn read_payload(
    reader: &mut (impl tokio::io::AsyncRead + Unpin),
    payload: &mut [u8],
    header: Header,
) -> anyhow::Result<()> {
    let _time = prysm_metrics::timer(Stage::FrameReadPayload);
    reader
        .read_exact(payload)
        .await
        .inspect_err(|error| {
            prysm_metrics::event(if error.kind() == std::io::ErrorKind::UnexpectedEof {
                FrameReceiverEvent::PayloadTruncated
            } else {
                FrameReceiverEvent::PayloadReadFailed
            });
        })
        .context("reading webOS frame payload")?;
    prysm_metrics::wire_frame(
        WireBoundary::PayloadReceived,
        header.format.into(),
        Codec::from(&header),
        prysm_metrics::Content::Unspecified,
        header.width,
        header.height,
    );
    prysm_metrics::bytes(BytesKind::ReceiverPayload, payload.len());
    Ok(())
}

async fn read_pixels(
    reader: &mut (impl tokio::io::AsyncRead + Unpin),
    header: Header,
    scratch: &mut [Vec<u8>; 2],
) -> anyhow::Result<Frame> {
    if header.format == WireFormat::Rgb {
        let mut data = vec![0; header.raw_len()?];
        if header.codec == prysm_webos_protocol::Codec::Lz4 {
            scratch[0].resize(header.payload_len, 0);
            read_payload(reader, &mut scratch[0], header).await?;
            let _time = prysm_metrics::timer(Stage::FrameDecompress);
            prysm_webos_protocol::decompress(&scratch[0], &mut data)
                .inspect_err(|_| prysm_metrics::event(DecoderEvent::DecompressionFailed))?;
        } else {
            read_payload(reader, &mut data, header).await?;
        }
        return Ok(Frame::new(
            data,
            header.width,
            header.height,
            PixelFormat::RGB24,
        )?);
    }
    let [payload, decoded] = scratch;
    payload.resize(header.payload_len, 0);
    read_payload(reader, payload, header).await?;
    let raw = if header.codec == prysm_webos_protocol::Codec::Lz4 {
        decoded.resize(header.raw_len()?, 0);
        let _time = prysm_metrics::timer(Stage::FrameDecompress);
        prysm_webos_protocol::decompress(payload, decoded)
            .inspect_err(|_| prysm_metrics::event(DecoderEvent::DecompressionFailed))?;
        decoded.as_slice()
    } else {
        payload.as_slice()
    };
    let data = {
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
        let mut rgb = vec![0; header.width as usize * header.height as usize * 3];
        let convert = match header.format {
            WireFormat::Nv12 => yuv::yuv_nv12_to_rgb,
            WireFormat::Nv16 => yuv::yuv_nv16_to_rgb,
            WireFormat::Rgb => unreachable!(),
        };
        convert(
            &image,
            &mut rgb,
            header.width * 3,
            yuv::YuvRange::Limited,
            yuv::YuvStandardMatrix::Bt709,
            yuv::YuvConversionMode::Balanced,
        )
        .inspect_err(|_| prysm_metrics::event(DecoderEvent::ConversionFailed))?;
        rgb
    };
    Ok(Frame::new(
        data,
        header.width,
        header.height,
        PixelFormat::RGB24,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

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
        assert!(read_frame(&mut malformed.as_slice()).await.is_err());
        let mut rejected = fixture_packet(WireFormat::Rgb, false);
        rejected[0] = 0;
        assert!(read_frame(&mut rejected.as_slice()).await.is_err());
        let truncated = &fixture_packet(WireFormat::Rgb, false)[..=HEADER_LEN];
        assert!(read_frame(&mut &truncated[..]).await.is_err());
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
        let (mut writer, mut reader) = tokio::io::duplex(7);
        let send = tokio::spawn(async move {
            for format in [WireFormat::Rgb, WireFormat::Nv12, WireFormat::Nv16] {
                // Tiny chroma frames may expand under LZ4, so raw is sufficient
                // here; compressed NV12/NV16 are covered below at a larger size.
                let variants: &[bool] = if format == WireFormat::Rgb {
                    &[false, true]
                } else {
                    &[false]
                };
                for &compressed in variants {
                    for fragment in fixture_packet(format, compressed).chunks(3) {
                        writer.write_all(fragment).await.unwrap();
                    }
                }
            }
            writer
                .write_all(&raw_rgb_packet(1, 1, &[1, 2, 3]))
                .await
                .unwrap();
        });
        let mut scratch = [Vec::new(), Vec::new()];
        for expected in [
            vec![42; 24],
            vec![42; 24],
            vec![0; 24],
            vec![0; 24],
            vec![1, 2, 3],
        ] {
            assert_eq!(
                read_frame_into(&mut reader, &mut scratch, &mut None)
                    .await
                    .unwrap()
                    .as_slice(),
                expected
            );
        }
        send.await.unwrap();
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
                    let frame = read_frame(&mut packet.as_slice()).await.unwrap();
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
                read_frame(&mut packet.as_slice())
                    .await
                    .unwrap()
                    .as_slice()
                    .iter()
                    .all(|v| *v <= 1)
            );
            packet[HEADER_LEN] = 0xff;
            assert!(read_frame(&mut packet.as_slice()).await.is_err());
        }
        let mut packet = fixture_packet(WireFormat::Rgb, false);
        for (offset, value) in [(4, u32::MAX), (12, 99), (16, 99), (20, u32::MAX)] {
            let mut bad = packet.clone();
            bad[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
            assert!(read_frame(&mut bad.as_slice()).await.is_err());
        }
        packet.pop();
        assert!(read_frame(&mut packet.as_slice()).await.is_err());
    }

    #[tokio::test]
    async fn fragmented_and_consecutive_rgb_frames_keep_their_boundaries() {
        let (mut writer, mut reader) = tokio::io::duplex(8);
        let send = tokio::spawn(async move {
            for bytes in [
                raw_rgb_packet(1, 1, &[255, 0, 12]),
                raw_rgb_packet(3, 1, &[42; 9]),
            ] {
                for fragment in bytes.chunks(3) {
                    writer.write_all(fragment).await.unwrap();
                }
            }
        });
        let first = read_frame(&mut reader).await.unwrap();
        assert_eq!(
            (first.width(), first.height(), first.format()),
            (1, 1, PixelFormat::RGB24)
        );
        assert_eq!(first.as_slice(), &[255, 0, 12]);
        assert_eq!(read_frame(&mut reader).await.unwrap().as_slice(), &[42; 9]);
        send.await.unwrap();
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
            assert!(read_frame(&mut bytes.as_slice()).await.is_err());
        }
    }

    #[tokio::test]
    async fn h264_packets_decode_in_order_with_reused_connection_state() {
        let mut encoder = prysm_webos_protocol::h264::Encoder::new(2_000_000, 30).unwrap();
        let mut decoder = None;
        let mut scratch = [Vec::new(), Vec::new()];
        let header = Header {
            width: 64,
            height: 32,
            format: WireFormat::Rgb,
            codec: prysm_webos_protocol::Codec::Raw,
            payload_len: 64 * 32 * 3,
        };
        for _ in 0..3 {
            let mut payload = Vec::new();
            encoder
                .encode(header, &[255, 0, 0].repeat(64 * 32), &mut payload)
                .unwrap();
            let wire = Header {
                codec: prysm_webos_protocol::Codec::H264,
                payload_len: payload.len(),
                ..header
            };
            let packet = [wire.encode().unwrap().as_slice(), &payload].concat();
            let frame = read_frame_into(&mut packet.as_slice(), &mut scratch, &mut decoder)
                .await
                .unwrap();
            assert_eq!(frame.width(), 64);
            assert_eq!(frame.height(), 32);
            assert!(decoder.is_some());
        }
    }
}
