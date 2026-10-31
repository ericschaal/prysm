//! Receive raw/LZ4 RGB24, NV12 or NV16 frames from the Rust webOS sender.
use crate::{Frame, PixelFormat, PrysmCapturer, Result, capture_channel};
use anyhow::Context;
use futures::Stream;
use prysm_webos_protocol::{HEADER_LEN, Header, PixelFormat as WireFormat};
use std::{net::SocketAddr, time::Duration};
use tokio::{io::AsyncReadExt, net::TcpListener, sync::watch};
use tokio_util::sync::CancellationToken;

/// A session-scoped listener. TV disconnects return to accepting a connection.
///
/// This protocol has no authentication: bind on a trusted LAN interface only.
/// Incoming dimensions are authoritative; `into_stream` dimensions are unused.
pub struct WebosCapturer {
    listener: std::net::TcpListener,
    shutdown: CancellationToken,
    completed: CancellationToken,
}

impl WebosCapturer {
    /// Bind immediately so address conflicts are reported during pipeline startup.
    pub fn new(listen: SocketAddr, shutdown: CancellationToken) -> Result<Self> {
        let listener = std::net::TcpListener::bind(listen)
            .with_context(|| format!("cannot listen for webOS capture on {listen}"))?;
        listener
            .set_nonblocking(true)
            .context("cannot configure webOS listener")?;
        Ok(Self {
            listener,
            shutdown,
            completed: CancellationToken::new(),
        })
    }

    #[must_use]
    pub fn completion(&self) -> CancellationToken {
        self.completed.clone()
    }
}

impl PrysmCapturer for WebosCapturer {
    fn into_stream(self, _: u32, _: u32) -> impl Stream<Item = Result<Frame>> + Send + 'static {
        let (sender, finished, frames) = capture_channel(self.shutdown.clone());
        tokio::spawn(async move {
            let _completed = self.completed.drop_guard();
            let result = async {
                let listener = TcpListener::from_std(self.listener)?;
                tokio::select! {
                    () = self.shutdown.cancelled() => Ok(()),
                    () = sender.closed() => Ok(()),
                    result = receive(&listener, &sender) => result,
                }
            }
            .await;
            let _ = finished.send(result.map_err(Into::into));
        });
        frames
    }
}

async fn receive(
    listener: &TcpListener,
    sender: &watch::Sender<Option<Frame>>,
) -> anyhow::Result<()> {
    loop {
        let (mut socket, peer) = listener.accept().await.context("webOS accept failed")?;
        tracing::info!(%peer, "webOS capture connected");
        let mut scratch = [Vec::new(), Vec::new()];
        loop {
            // A stalled or partial frame must not hold the only TV connection forever.
            match tokio::time::timeout(
                Duration::from_secs(5),
                read_frame_into(&mut socket, &mut scratch),
            )
            .await
            {
                Ok(Ok(frame)) => {
                    if sender.send(Some(frame)).is_err() {
                        return Ok(());
                    }
                }
                Ok(Err(error)) => {
                    tracing::debug!(%peer, error = %format!("{error:#}"), "webOS connection ended");
                    break;
                }
                Err(_) => {
                    tracing::warn!(%peer, "webOS frame timed out");
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
async fn read_frame(reader: &mut (impl tokio::io::AsyncRead + Unpin)) -> anyhow::Result<Frame> {
    read_frame_into(reader, &mut [Vec::new(), Vec::new()]).await
}

async fn read_frame_into(
    reader: &mut (impl tokio::io::AsyncRead + Unpin),
    scratch: &mut [Vec<u8>; 2],
) -> anyhow::Result<Frame> {
    let mut header = [0; HEADER_LEN];
    reader
        .read_exact(&mut header)
        .await
        .context("reading webOS frame header")?;
    read_pixels(reader, Header::decode(&header)?, scratch).await
}

async fn read_pixels(
    reader: &mut (impl tokio::io::AsyncRead + Unpin),
    header: Header,
    scratch: &mut [Vec<u8>; 2],
) -> anyhow::Result<Frame> {
    if header.format == WireFormat::Rgb {
        let mut data = vec![0; header.raw_len()?];
        if header.compressed {
            scratch[0].resize(header.payload_len, 0);
            reader
                .read_exact(&mut scratch[0])
                .await
                .context("reading webOS LZ4 pixels")?;
            prysm_webos_protocol::decompress(&scratch[0], &mut data)?;
        } else {
            reader
                .read_exact(&mut data)
                .await
                .context("reading webOS RGB pixels")?;
        }
        return Ok(Frame::new(
            data,
            header.width,
            header.height,
            PixelFormat::RGB24,
        ));
    }
    let [payload, decoded] = scratch;
    payload.resize(header.payload_len, 0);
    reader
        .read_exact(payload)
        .await
        .context("reading webOS v2 pixels")?;
    let raw = if header.compressed {
        decoded.resize(header.raw_len()?, 0);
        prysm_webos_protocol::decompress(payload, decoded)?;
        decoded.as_slice()
    } else {
        payload.as_slice()
    };
    let data = {
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
            yuv::YuvStandardMatrix::Bt601,
            yuv::YuvConversionMode::Fast,
        )?;
        rgb
    };
    Ok(Frame::new(
        data,
        header.width,
        header.height,
        PixelFormat::RGB24,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use tokio::{io::AsyncWriteExt, net::TcpStream};

    fn packet(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
        let mut packet = b"PRY2".to_vec();
        for value in [width, height, 0, 0, u32::try_from(pixels.len()).unwrap()] {
            packet.extend_from_slice(&value.to_be_bytes());
        }
        packet.extend_from_slice(pixels);
        packet
    }

    fn v2_packet(format: WireFormat, compressed: bool) -> Vec<u8> {
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
            compressed,
            payload_len: payload.len(),
        }
        .encode()
        .unwrap()
        .to_vec();
        packet.extend_from_slice(payload);
        packet
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
                    for fragment in v2_packet(format, compressed).chunks(3) {
                        writer.write_all(fragment).await.unwrap();
                    }
                }
            }
            writer.write_all(&packet(1, 1, &[1, 2, 3])).await.unwrap();
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
                read_frame_into(&mut reader, &mut scratch)
                    .await
                    .unwrap()
                    .as_slice(),
                expected
            );
        }
        send.await.unwrap();
    }

    #[tokio::test]
    async fn compressed_chroma_decodes_and_malformed_blocks_disconnect() {
        for format in [WireFormat::Nv12, WireFormat::Nv16] {
            let mut header = Header {
                width: 32,
                height: 16,
                format,
                compressed: false,
                payload_len: 0,
            };
            let raw = [vec![16; 512], vec![128; header.raw_len().unwrap() - 512]].concat();
            let mut scratch = Vec::new();
            let length = prysm_webos_protocol::compress(&raw, &mut scratch)
                .unwrap()
                .unwrap();
            header.compressed = true;
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
        let mut packet = v2_packet(WireFormat::Rgb, false);
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
            for bytes in [packet(1, 1, &[255, 0, 12]), packet(3, 1, &[42; 9])] {
                for fragment in bytes.chunks(3) {
                    writer.write_all(fragment).await.unwrap();
                }
            }
        });
        let first = read_frame(&mut reader).await.unwrap();
        assert_eq!(
            (first.width, first.height, first.format),
            (1, 1, PixelFormat::RGB24)
        );
        assert_eq!(first.as_slice(), &[255, 0, 12]);
        assert_eq!(read_frame(&mut reader).await.unwrap().as_slice(), &[42; 9]);
        send.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_bad_headers_and_truncated_pixels_without_panicking() {
        let mut bad_magic = packet(1, 1, &[0; 3]);
        bad_magic[0] = 0;
        for bytes in [
            bad_magic,
            packet(0, 1, &[]),
            packet(u32::MAX, u32::MAX, &[]),
            packet(1921, 1, &[]),
            packet(1, 1081, &[]),
            packet(2, 2, &[0; 3]),
            packet(1, 1, &[0; 3])[..HEADER_LEN + 2].to_vec(),
        ] {
            assert!(read_frame(&mut bytes.as_slice()).await.is_err());
        }
    }

    #[tokio::test]
    async fn reconnects_after_bad_client_and_releases_listener_on_stop_or_drop() {
        for stop in [true, false] {
            let shutdown = CancellationToken::new();
            let capturer =
                WebosCapturer::new("127.0.0.1:0".parse().unwrap(), shutdown.clone()).unwrap();
            let address = capturer.listener.local_addr().unwrap();
            let completed = capturer.completion();
            let mut frames = Box::pin(capturer.into_stream(640, 360));
            let mut bad = TcpStream::connect(address).await.unwrap();
            bad.write_all(&[0; 16]).await.unwrap();
            drop(bad);
            let mut client = TcpStream::connect(address).await.unwrap();
            client.write_all(&packet(1, 1, &[1, 2, 3])).await.unwrap();
            let frame = tokio::time::timeout(Duration::from_secs(1), frames.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(frame.as_slice(), &[1, 2, 3]);
            // Cancellation also interrupts an incomplete payload.
            client
                .write_all(&packet(1, 1, &[4, 5, 6])[..HEADER_LEN + 1])
                .await
                .unwrap();
            if stop {
                shutdown.cancel();
                assert!(frames.next().await.is_none());
            }
            drop(frames);
            tokio::time::timeout(Duration::from_secs(1), completed.cancelled())
                .await
                .unwrap();
            assert!(std::net::TcpListener::bind(address).is_ok());
        }
    }

    #[test]
    fn bind_conflict_is_a_startup_error() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        assert!(
            WebosCapturer::new(listener.local_addr().unwrap(), CancellationToken::new()).is_err()
        );
    }
}
