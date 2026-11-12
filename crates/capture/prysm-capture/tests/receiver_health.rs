#![cfg(feature = "remote")]

//! Exercise exported health metrics through a real receiver connection and capture lifecycle.
use futures::{SinkExt, StreamExt};
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
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;

// Each receiver writes the same process-wide capture gauges.
static RECEIVER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

async fn endpoint(receiver: &WebosReceiver) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/api/capture", listener.local_addr().unwrap());
    let router = receiver.router();
    tokio::spawn(async move {
        axum::serve(listener, router).await;
    });
    url
}
type Tv = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;
async fn connect(url: &str) -> Tv {
    let (mut tv, _) = connect_async(url).await.unwrap();
    tv.send(Message::binary(HELLO.encode().unwrap()))
        .await
        .unwrap();
    tv
}
async fn reply(tv: &mut Tv) -> ControlMessage {
    ControlMessage::decode(&tv.next().await.unwrap().unwrap().into_data()).unwrap()
}

#[tokio::test]
async fn health_distinguishes_idle_waiting_receiving_reconnecting_and_stopped() {
    let _guard = RECEIVER.lock().await;
    tokio::time::timeout(Duration::from_secs(8), async {
        let exporter =
            prysm_metrics::start("127.0.0.1:0".parse().unwrap(), prysm_metrics::Role::Prysm)
                .unwrap();
        let address = exporter.address();
        let last = "prysm_receiver_last_decoded_frame_timestamp_seconds";
        let receiver = WebosReceiver::new().unwrap();
        assert_eq!(metric(address, last).await, 0.0);
        requested(address, 0.0).await;
        let shutdown = CancellationToken::new();
        let (frames, completed, _) = receiver
            .capture(4, 2, 30.try_into().unwrap(), shutdown.clone())
            .unwrap();
        let mut frames = Box::pin(frames);
        requested(address, 30.0).await;
        assert_eq!(metric(address, last).await, 0.0);
        let mut tv = connect(&endpoint(&receiver).await).await;
        assert!(matches!(
            reply(&mut tv).await,
            ControlMessage::CaptureSettings { .. }
        ));
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
        let packet = [header.as_slice(), &[42; 24]].concat();
        tv.send(Message::binary(packet)).await.unwrap();
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
        receiver.shutdown().await.unwrap();
        completed.cancelled().await;
        requested(address, 0.0).await;
        drop(frames);
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn ready_tvs_are_ordered_and_an_active_session_survives_a_busy_handshake() {
    let _guard = RECEIVER.lock().await;
    tokio::time::timeout(Duration::from_secs(4), async {
        let receiver = WebosReceiver::new().unwrap();
        let url = endpoint(&receiver).await;
        let mut first = connect(&url).await;
        assert_eq!(reply(&mut first).await, ControlMessage::Idle);
        let mut second = connect(&url).await;
        assert_eq!(reply(&mut second).await, ControlMessage::Idle);
        let stop = CancellationToken::new();
        let (_frames, completed, _) = receiver
            .capture(4, 2, 30.try_into().unwrap(), stop.clone())
            .unwrap();
        assert!(matches!(
            reply(&mut first).await,
            ControlMessage::CaptureSettings { .. }
        ));
        assert_eq!(reply(&mut second).await, ControlMessage::Busy);
        assert_eq!(reply(&mut connect(&url).await).await, ControlMessage::Busy);
        first.send(Message::binary(vec![1, 2, 3])).await.unwrap();
        assert!(
            first
                .next()
                .await
                .is_none_or(|m| m.is_err() || m.unwrap().is_close())
        );
        let mut replacement = connect(&url).await;
        assert!(matches!(
            reply(&mut replacement).await,
            ControlMessage::CaptureSettings { .. }
        ));
        stop.cancel();
        completed.cancelled().await;
        assert_eq!(reply(&mut connect(&url).await).await, ControlMessage::Idle);
        receiver.shutdown().await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn idle_waiters_keep_their_place_across_renewals_and_video_loss() {
    let _guard = RECEIVER.lock().await;
    tokio::time::timeout(Duration::from_secs(4), async {
        let receiver = WebosReceiver::new().unwrap();
        let url = endpoint(&receiver).await;
        let mut first = connect(&url).await;
        assert_eq!(reply(&mut first).await, ControlMessage::Idle);
        let mut second = connect(&url).await;
        assert_eq!(reply(&mut second).await, ControlMessage::Idle);
        assert_eq!(reply(&mut first).await, ControlMessage::Idle);
        assert_eq!(reply(&mut second).await, ControlMessage::Idle);
        drop(first);
        // Allow the receiver to observe the departure before requesting capture.
        assert_eq!(reply(&mut second).await, ControlMessage::Idle);
        let stop = CancellationToken::new();
        let (_frames, completed, _) = receiver
            .capture(4, 2, 30.try_into().unwrap(), stop.clone())
            .unwrap();
        assert!(matches!(
            reply(&mut second).await,
            ControlMessage::CaptureSettings { .. }
        ));
        stop.cancel();
        completed.cancelled().await;
        receiver.shutdown().await.unwrap();
    })
    .await
    .unwrap();
}
