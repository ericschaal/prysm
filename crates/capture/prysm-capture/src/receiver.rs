//! Application-owned webOS listener; capture requests do not own the listening socket.
use crate::{Frame, Result, capture_channel};
use anyhow::{Context, ensure};
use futures::Stream;
use prysm_webos_protocol::{ControlMessage, HELLO};
use std::{net::SocketAddr, num::NonZeroU32, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot, watch},
};
use tokio_util::sync::{CancellationToken, DropGuard};

/// A worker owns LAN reception and decoding, independently of capture and API runtimes.
/// The last handle closing stops the worker; the wire protocol is unauthenticated.
#[derive(Clone)]
pub struct WebosReceiver {
    address: SocketAddr,
    requests: mpsc::Sender<Request>,
}

struct Request {
    settings: ControlMessage,
    receiving: watch::Sender<bool>,
    frames: watch::Sender<Option<Frame>>,
    finished: oneshot::Sender<Result<()>>,
    shutdown: CancellationToken,
    _completed: DropGuard,
}

impl WebosReceiver {
    /// Bind at application startup, including while capture is off.
    pub fn new(address: SocketAddr) -> Result<Self> {
        let listener = std::net::TcpListener::bind(address)
            .with_context(|| format!("cannot listen for webOS on {address}"))?;
        let address = listener
            .local_addr()
            .context("cannot read webOS listener address")?;
        listener
            .set_nonblocking(true)
            .context("cannot configure webOS listener")?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("cannot create webOS receiver runtime")?;
        let (requests, incoming) = mpsc::channel(1);
        std::thread::Builder::new()
            .name("prysm-webos-receiver".into())
            .spawn(move || {
                let result = runtime.block_on(async move {
                    let listener =
                        TcpListener::from_std(listener).context("cannot attach webOS listener")?;
                    serve(listener, incoming).await
                });
                if let Err(error) = result {
                    tracing::error!(error = %format!("{error:#}"), "webOS listener failed");
                }
            })
            .context("cannot start webOS receiver worker")?;
        Ok(Self { address, requests })
    }

    /// The bound listening address.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// One active pipeline requests frames; dropping/stopping it closes its TV session.
    /// Returns frames, a cleanup notification, and availability (true after receiving a frame).
    pub fn capture(
        &self,
        width: u32,
        height: u32,
        rate: NonZeroU32,
        shutdown: CancellationToken,
    ) -> Result<(
        impl Stream<Item = Result<Frame>> + Send + 'static,
        CancellationToken,
        watch::Receiver<bool>,
    )> {
        let settings = ControlMessage::CaptureSettings {
            refresh_rate_hz: rate.get(),
            capture_resolution: [width, height],
        }
        .validate()?;
        let (frames, finished, stream) = capture_channel(shutdown.clone());
        let completed = CancellationToken::new();
        let (receiving, availability) = watch::channel(false);
        self.requests
            .try_send(Request {
                settings,
                receiving,
                frames,
                finished,
                shutdown,
                _completed: completed.clone().drop_guard(),
            })
            .context("webOS listener unavailable or capture already pending")?;
        Ok((stream, completed, availability))
    }
}

async fn stop_requested(active: &Option<Request>) {
    match active {
        Some(request) => tokio::select! {
            () = request.shutdown.cancelled() => {},
            () = request.frames.closed() => {},
        },
        None => std::future::pending().await,
    }
}

fn finish(active: &mut Option<Request>) {
    if let Some(request) = active.take() {
        request.receiving.send_replace(false);
        let _ = request.finished.send(Ok(()));
    }
}

async fn serve(listener: TcpListener, mut requests: mpsc::Receiver<Request>) -> anyhow::Result<()> {
    let mut active = None;
    loop {
        tokio::select! {
            biased;
            () = stop_requested(&active) => finish(&mut active),
            request = requests.recv() => match request {
                Some(request) => { ensure!(active.is_none(), "overlapping webOS captures"); active = Some(request); },
                None => return Ok(()),
            },
            accepted = accept(&listener) => {
                let (socket, peer) = accepted?;
                if let Err(error) = connection(socket, peer, &listener, &mut active, &mut requests).await {
                    tracing::debug!(%peer, error = %format!("{error:#}"), "webOS connection ended");
                }
                if requests.is_closed() { return Ok(()); }
            }
        }
    }
}

async fn accept(listener: &TcpListener) -> anyhow::Result<(TcpStream, SocketAddr)> {
    loop {
        let (mut socket, peer) = listener.accept().await.context("webOS accept failed")?;
        let handshake = tokio::time::timeout(Duration::from_secs(1), async {
            let mut prefix = [0; 4];
            socket.read_exact(&mut prefix).await?;
            let length = ControlMessage::decode_length(prefix)?;
            let mut bytes = vec![0; length];
            socket.read_exact(&mut bytes).await?;
            ensure!(
                ControlMessage::decode(&bytes)? == HELLO,
                "expected webOS hello"
            );
            anyhow::Ok(())
        })
        .await;
        if !matches!(handshake, Ok(Ok(()))) {
            tracing::debug!(%peer, ?handshake, "webOS handshake failed");
            continue;
        }
        prysm_metrics::event(prysm_metrics::FrameReceiverEvent::ConnectionAccepted);
        return Ok((socket, peer));
    }
}

async fn connection(
    mut socket: TcpStream,
    mut peer: SocketAddr,
    listener: &TcpListener,
    active: &mut Option<Request>,
    requests: &mut mpsc::Receiver<Request>,
) -> anyhow::Result<()> {
    if active.is_none() {
        socket.write_all(&ControlMessage::Idle.encode()?).await?;
    }
    while active.is_none() {
        let mut byte = [0];
        tokio::select! {
            request = requests.recv() => match request { Some(request) => *active = Some(request), None => return Ok(()) },
            _ = socket.read(&mut byte) => return Ok(()),
            replacement = accept(listener) => {
                let (next, address) = replacement?;
                socket = next;
                peer = address;
                socket.write_all(&ControlMessage::Idle.encode()?).await?;
            },
        }
    }
    let request = active.as_ref().expect("capture requested");
    if request.shutdown.is_cancelled() || request.frames.is_closed() {
        finish(active);
        return Ok(());
    }
    socket.write_all(&request.settings.encode()?).await?;
    let mut last = None;
    tokio::select! {
        () = request.shutdown.cancelled() => {},
        () = request.frames.closed() => {},
        next = requests.recv() => { ensure!(next.is_none(), "overlapping webOS captures"); },
        () = crate::webos::receive_frames(&mut socket, &request.frames, &request.receiving, peer, &mut last) => {
            request.receiving.send_replace(false);
            return Ok(());
        },
    }
    finish(active);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use prysm_webos_protocol::HEADER_LEN;

    async fn control(socket: &mut TcpStream) -> ControlMessage {
        let mut prefix = [0; 4];
        socket.read_exact(&mut prefix).await.unwrap();
        let mut bytes = vec![0; ControlMessage::decode_length(prefix).unwrap()];
        socket.read_exact(&mut bytes).await.unwrap();
        ControlMessage::decode(&bytes).unwrap()
    }

    #[tokio::test]
    async fn a_valid_replacement_takes_over_an_idle_connection() {
        tokio::time::timeout(Duration::from_secs(3), async {
            let receiver = WebosReceiver::new("127.0.0.1:0".parse().unwrap()).unwrap();
            let mut old = TcpStream::connect(receiver.address()).await.unwrap();
            old.write_all(&HELLO.encode().unwrap()).await.unwrap();
            assert_eq!(control(&mut old).await, ControlMessage::Idle);

            let mut invalid = TcpStream::connect(receiver.address()).await.unwrap();
            invalid.write_all(&[0; 4]).await.unwrap();
            assert_eq!(invalid.read(&mut [0]).await.unwrap(), 0);
            assert!(old.try_read(&mut [0]).is_err());

            let mut replacement = TcpStream::connect(receiver.address()).await.unwrap();
            replacement
                .write_all(&HELLO.encode().unwrap())
                .await
                .unwrap();
            assert_eq!(control(&mut replacement).await, ControlMessage::Idle);
            assert_eq!(old.read(&mut [0]).await.unwrap(), 0);
            let (frames, completed, _availability) = receiver
                .capture(4, 2, 30.try_into().unwrap(), CancellationToken::new())
                .unwrap();
            assert!(matches!(
                control(&mut replacement).await,
                ControlMessage::CaptureSettings { .. }
            ));
            drop(frames);
            completed.cancelled().await;
        })
        .await
        .unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn receiver_progress_does_not_depend_on_the_callers_runtime() {
        let receiver = WebosReceiver::new("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = receiver.address();
        let client = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let mut socket = std::net::TcpStream::connect(address).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            socket.write_all(&HELLO.encode().unwrap()).unwrap();
            let mut prefix = [0; 4];
            socket.read_exact(&mut prefix)?;
            let mut bytes = vec![0; ControlMessage::decode_length(prefix).unwrap()];
            socket.read_exact(&mut bytes)?;
            assert_eq!(
                ControlMessage::decode(&bytes).unwrap(),
                ControlMessage::Idle
            );
            std::io::Result::Ok(())
        });
        // Deliberately block this runtime: reception must belong to the receiver worker.
        client.join().unwrap().unwrap();
        drop(receiver);
        released(address).await;
    }

    #[tokio::test]
    async fn idle_start_stop_and_reconnect_keep_the_application_listener() {
        tokio::time::timeout(Duration::from_secs(5), async {
            let receiver = WebosReceiver::new("127.0.0.1:0".parse().unwrap()).unwrap();
            // :0 is useful to callers/tests too; the receiver must expose the bound address.
            let address = receiver.address();
            for rate in [15, 30] {
                let mut tv = TcpStream::connect(address).await.unwrap();
                tv.write_all(&HELLO.encode().unwrap()).await.unwrap();
                assert_eq!(control(&mut tv).await, ControlMessage::Idle);
                let shutdown = CancellationToken::new();
                let (frames, completed, _availability) = receiver
                    .capture(4, 2, rate.try_into().unwrap(), shutdown.clone())
                    .unwrap();
                let mut frames = Box::pin(frames);
                assert_eq!(
                    control(&mut tv).await,
                    ControlMessage::CaptureSettings {
                        refresh_rate_hz: rate,
                        capture_resolution: [4, 2]
                    }
                );
                let header = prysm_webos_protocol::Header {
                    width: 4,
                    height: 2,
                    format: prysm_webos_protocol::PixelFormat::Rgb,
                    codec: prysm_webos_protocol::Codec::Raw,
                    payload_len: 24,
                };
                tv.write_all(&header.encode().unwrap()).await.unwrap();
                tv.write_all(&[42; 24]).await.unwrap();
                assert_eq!(frames.next().await.unwrap().unwrap().as_slice(), &[42; 24]);
                shutdown.cancel();
                assert!(frames.next().await.is_none());
                completed.cancelled().await;
                assert_eq!(tv.read(&mut [0]).await.unwrap(), 0);
            }
            let mut tv = TcpStream::connect(address).await.unwrap();
            tv.write_all(&HELLO.encode().unwrap()).await.unwrap();
            assert_eq!(control(&mut tv).await, ControlMessage::Idle);
            drop(receiver);
            assert_eq!(tv.read(&mut [0]).await.unwrap(), 0);
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn stopping_before_a_tv_connects_completes_and_keeps_the_listener() {
        tokio::time::timeout(Duration::from_secs(2), async {
            let receiver = WebosReceiver::new("127.0.0.1:0".parse().unwrap()).unwrap();
            let shutdown = CancellationToken::new();
            let (frames, completed, _availability) = receiver
                .capture(4, 2, 30.try_into().unwrap(), shutdown.clone())
                .unwrap();
            shutdown.cancel();
            drop(frames);
            completed.cancelled().await;
            let mut tv = TcpStream::connect(receiver.address()).await.unwrap();
            tv.write_all(&HELLO.encode().unwrap()).await.unwrap();
            assert_eq!(control(&mut tv).await, ControlMessage::Idle);
        })
        .await
        .unwrap();
    }

    async fn released(address: SocketAddr) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while std::net::TcpListener::bind(address).is_err() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    fn raw_rgb_packet(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
        let mut packet = prysm_webos_protocol::Header {
            width,
            height,
            format: prysm_webos_protocol::PixelFormat::Rgb,
            codec: prysm_webos_protocol::Codec::Raw,
            payload_len: pixels.len(),
        }
        .encode()
        .unwrap()
        .to_vec();
        packet.extend_from_slice(pixels);
        packet
    }

    #[test]
    fn bind_conflict_is_a_startup_error() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        assert!(WebosReceiver::new(listener.local_addr().unwrap()).is_err());
    }

    #[tokio::test]
    async fn bad_clients_reconnect_and_stop_or_drop_releases_the_capture() {
        for (stop, rate) in [(true, 15), (false, 60)] {
            let shutdown = CancellationToken::new();
            let receiver = WebosReceiver::new("127.0.0.1:0".parse().unwrap()).unwrap();
            let address = receiver.address();
            let (frames, completed, _availability) = receiver
                .capture(640, 360, rate.try_into().unwrap(), shutdown.clone())
                .unwrap();
            let mut frames = Box::pin(frames);
            let mut bad = TcpStream::connect(address).await.unwrap();
            bad.write_all(&[0; 16]).await.unwrap();
            drop(bad);
            let mut client = TcpStream::connect(address).await.unwrap();
            for byte in HELLO.encode().unwrap() {
                client.write_all(&[byte]).await.unwrap();
            }
            let mut prefix = [0; 4];
            client.read_exact(&mut prefix).await.unwrap();
            let length = ControlMessage::decode_length(prefix).unwrap();
            let mut settings = vec![0; length];
            client.read_exact(&mut settings).await.unwrap();
            assert_eq!(
                ControlMessage::decode(&settings).unwrap(),
                ControlMessage::CaptureSettings {
                    refresh_rate_hz: rate,
                    capture_resolution: [640, 360],
                }
            );
            client
                .write_all(&raw_rgb_packet(1, 1, &[1, 2, 3]))
                .await
                .unwrap();
            let frame = tokio::time::timeout(Duration::from_secs(1), frames.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(frame.as_slice(), &[1, 2, 3]);

            let mut malformed = raw_rgb_packet(1, 1, &[0; 3]);
            malformed[0] = 0;
            client.write_all(&malformed).await.unwrap();
            assert!(match client.read(&mut [0]).await {
                Ok(size) => size == 0,
                Err(error) => error.kind() == std::io::ErrorKind::ConnectionReset,
            });
            client = TcpStream::connect(address).await.unwrap();
            client.write_all(&HELLO.encode().unwrap()).await.unwrap();
            assert!(matches!(
                control(&mut client).await,
                ControlMessage::CaptureSettings { .. }
            ));
            client
                .write_all(&raw_rgb_packet(1, 1, &[7, 8, 9]))
                .await
                .unwrap();
            assert_eq!(frames.next().await.unwrap().unwrap().as_slice(), &[7, 8, 9]);

            // Cancellation also interrupts an incomplete payload.
            client
                .write_all(&raw_rgb_packet(1, 1, &[4, 5, 6])[..HEADER_LEN + 1])
                .await
                .unwrap();
            if stop {
                shutdown.cancel();
                assert!(frames.next().await.is_none());
            }
            drop(frames);
            tokio::time::timeout(Duration::from_millis(200), completed.cancelled())
                .await
                .unwrap();
            let mut idle = TcpStream::connect(address).await.unwrap();
            idle.write_all(&HELLO.encode().unwrap()).await.unwrap();
            assert_eq!(control(&mut idle).await, ControlMessage::Idle);
            drop(receiver);
            assert_eq!(idle.read(&mut [0]).await.unwrap(), 0);
            released(address).await;
        }
    }

    #[tokio::test]
    async fn cancellation_interrupts_a_stalled_handshake() {
        let shutdown = CancellationToken::new();
        let receiver = WebosReceiver::new("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = receiver.address();
        let (frames, completed, _availability) = receiver
            .capture(2, 2, 30.try_into().unwrap(), shutdown.clone())
            .unwrap();
        let mut frames = Box::pin(frames);
        let mut client = TcpStream::connect(address).await.unwrap();
        client
            .write_all(&HELLO.encode().unwrap()[..5])
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        shutdown.cancel();
        assert!(frames.next().await.is_none());
        drop(frames);
        tokio::time::timeout(Duration::from_millis(200), completed.cancelled())
            .await
            .unwrap();
        drop(receiver);
        released(address).await;
    }

    #[tokio::test]
    async fn raw_lz4_and_h264_frames_decode_across_tv_reconnections() {
        use prysm_webos_protocol::{Codec, Header, PixelFormat, h264::Encoder};

        tokio::time::timeout(Duration::from_secs(5), async {
            let receiver = WebosReceiver::new("127.0.0.1:0".parse().unwrap()).unwrap();
            let (frames, completed, _availability) = receiver
                .capture(64, 32, 30.try_into().unwrap(), CancellationToken::new())
                .unwrap();
            let mut frames = Box::pin(frames);
            let pixels = [255, 0, 0].repeat(64 * 32);
            for codec in [Codec::Raw, Codec::Lz4, Codec::H264, Codec::H264] {
                let mut tv = TcpStream::connect(receiver.address()).await.unwrap();
                tv.write_all(&HELLO.encode().unwrap()).await.unwrap();
                assert!(matches!(
                    control(&mut tv).await,
                    ControlMessage::CaptureSettings { .. }
                ));
                let mut header = Header {
                    width: 64,
                    height: 32,
                    format: PixelFormat::Rgb,
                    codec: Codec::Raw,
                    payload_len: pixels.len(),
                };
                let mut encoded = Vec::new();
                let payload = match codec {
                    Codec::Raw => pixels.as_slice(),
                    Codec::Lz4 => {
                        let length = prysm_webos_protocol::compress(&pixels, &mut encoded)
                            .unwrap()
                            .unwrap();
                        &encoded[..length]
                    }
                    Codec::H264 => {
                        Encoder::new(2_000_000, 30)
                            .unwrap()
                            .encode(header, &pixels, &mut encoded)
                            .unwrap()
                            .unwrap();
                        encoded.as_slice()
                    }
                };
                header.codec = codec;
                header.payload_len = payload.len();
                tv.write_all(&header.encode().unwrap()).await.unwrap();
                tv.write_all(payload).await.unwrap();
                let frame = frames.next().await.unwrap().unwrap();
                assert_eq!((frame.width(), frame.height()), (64, 32));
                if codec == Codec::H264 {
                    assert!(
                        frame
                            .as_slice()
                            .chunks_exact(3)
                            .all(|rgb| rgb[0] > 240 && rgb[1] < 15 && rgb[2] < 15)
                    );
                } else {
                    assert_eq!(frame.as_slice(), pixels);
                }
            }
            drop(frames);
            completed.cancelled().await;
        })
        .await
        .unwrap();
    }
}
