//! Exercise exported health metrics through a real receiver connection and capture lifecycle.
use futures::StreamExt;
use prysm_capture::WebosReceiver;
use prysm_webos_protocol::{Codec, ControlMessage, HELLO, Header, PixelFormat};
use std::{
    net::SocketAddr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_util::sync::CancellationToken;

async fn metric(address: SocketAddr, name: &str) -> f64 {
    let mut socket = TcpStream::connect(address).await.unwrap();
    socket
        .write_all(b"GET /metrics HTTP/1.0\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).await.unwrap();
    let line = response
        .lines()
        .find(|line| line.starts_with(&format!("{name}{{")))
        .unwrap();
    line.split_whitespace().last().unwrap().parse().unwrap()
}

async fn requested(address: SocketAddr, fps: f64) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while (metric(address, "prysm_capture_requested_frames_per_second").await - fps).abs()
            > f64::EPSILON
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn health_distinguishes_idle_waiting_receiving_reconnecting_and_stopped() {
    tokio::time::timeout(Duration::from_secs(8), async {
        let exporter =
            prysm_metrics::start("127.0.0.1:0".parse().unwrap(), prysm_metrics::Role::Prysm)
                .unwrap();
        let address = exporter.address();
        let last = "prysm_receiver_last_decoded_frame_timestamp_seconds";
        let receiver = WebosReceiver::new("127.0.0.1:0".parse().unwrap()).unwrap();
        assert_eq!(metric(address, last).await, 0.0);
        requested(address, 0.0).await;
        let shutdown = CancellationToken::new();
        let (frames, completed, _) = receiver
            .capture(4, 2, 30.try_into().unwrap(), shutdown.clone())
            .unwrap();
        let mut frames = Box::pin(frames);
        requested(address, 30.0).await;
        assert_eq!(metric(address, last).await, 0.0);
        let mut tv = TcpStream::connect(receiver.address()).await.unwrap();
        tv.write_all(&HELLO.encode().unwrap()).await.unwrap();
        let mut prefix = [0; 4];
        tv.read_exact(&mut prefix).await.unwrap();
        let mut settings = vec![0; ControlMessage::decode_length(prefix).unwrap()];
        tv.read_exact(&mut settings).await.unwrap();
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let header = Header {
            width: 4,
            height: 2,
            format: PixelFormat::Rgb,
            codec: Codec::Raw,
            payload_len: 24,
        }
        .encode()
        .unwrap();
        tv.write_all(&header).await.unwrap();
        // An incomplete frame must not make the stream look fresh.
        tv.write_all(&[42; 12]).await.unwrap();
        assert_eq!(metric(address, last).await, 0.0);
        tv.write_all(&[42; 12]).await.unwrap();
        assert_eq!(frames.next().await.unwrap().unwrap().as_slice(), &[42; 24]);
        let timestamp = metric(address, last).await;
        assert!(timestamp >= before);
        drop(tv);
        // Request remains active through disconnects; the timestamp must not advance.
        tokio::time::sleep(Duration::from_millis(30)).await;
        requested(address, 30.0).await;
        assert_eq!(metric(address, last).await, timestamp);
        shutdown.cancel();
        completed.cancelled().await;
        requested(address, 0.0).await;
        let (frames, completed, _) = receiver
            .capture(4, 2, 15.try_into().unwrap(), CancellationToken::new())
            .unwrap();
        requested(address, 15.0).await;
        assert_eq!(
            metric(address, last).await,
            0.0,
            "new request cannot reuse old freshness"
        );
        drop(frames);
        completed.cancelled().await;
        requested(address, 0.0).await;
        let (frames, completed, _) = receiver
            .capture(4, 2, 60.try_into().unwrap(), CancellationToken::new())
            .unwrap();
        requested(address, 60.0).await;
        drop(receiver);
        completed.cancelled().await;
        requested(address, 0.0).await;
        drop(frames);
    })
    .await
    .unwrap();
}
