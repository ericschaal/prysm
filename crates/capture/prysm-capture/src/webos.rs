//! Receive RGB24 frames from the minimal Prysm webOS sender.
use crate::{Frame, PixelFormat, PrysmCapturer, Result, capture_channel};
use anyhow::{Context, ensure};
use futures::Stream;
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
        loop {
            // A stalled or partial frame must not hold the only TV connection forever.
            match tokio::time::timeout(Duration::from_secs(5), read_frame(&mut socket)).await {
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

async fn read_frame(reader: &mut (impl tokio::io::AsyncRead + Unpin)) -> anyhow::Result<Frame> {
    let mut header = [0; 16];
    reader
        .read_exact(&mut header)
        .await
        .context("reading webOS frame header")?;
    ensure!(&header[..4] == b"PRY1", "unsupported webOS frame protocol");
    let field =
        |offset| u32::from_be_bytes(header[offset..offset + 4].try_into().expect("header field"));
    let (width, height, length) = (field(4), field(8), field(12));
    ensure!(
        (1..=1920).contains(&width) && (1..=1080).contains(&height),
        "invalid webOS frame dimensions"
    );
    ensure!(
        length == width * height * 3,
        "invalid webOS RGB24 payload length"
    );
    let mut data = vec![0; length as usize];
    reader
        .read_exact(&mut data)
        .await
        .context("reading webOS RGB24 pixels")?;
    Ok(Frame::new(data, width, height, PixelFormat::RGB24))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use tokio::{io::AsyncWriteExt, net::TcpStream};

    fn packet(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
        let mut packet = b"PRY1".to_vec();
        for value in [width, height, u32::try_from(pixels.len()).unwrap()] {
            packet.extend_from_slice(&value.to_be_bytes());
        }
        packet.extend_from_slice(pixels);
        packet
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
            packet(1, 1, &[0; 3])[..18].to_vec(),
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
                .write_all(&packet(1, 1, &[4, 5, 6])[..17])
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
