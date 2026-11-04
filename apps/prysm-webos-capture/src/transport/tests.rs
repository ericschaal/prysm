use super::*;
use crate::encoder::{Encoder, H264_BITRATE};
use prysm_webos_protocol::{Compression, HEADER_LEN, Header, PixelFormat, decompress};
use std::net::TcpStream;
use std::{
    net::{SocketAddr, TcpListener},
    thread,
};
use tokio_tungstenite::tungstenite::{self, WebSocket};

pub(crate) fn hello(listener: &TcpListener) -> WebSocket<TcpStream> {
    let (tcp, _) = listener.accept().unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut socket = tungstenite::accept(tcp).unwrap();
    assert_eq!(
        ControlMessage::decode(&socket.read().unwrap().into_data()).unwrap(),
        HELLO
    );
    socket
}
pub(crate) fn accept(listener: &TcpListener, rate: u32) -> WebSocket<TcpStream> {
    accept_resolution(listener, rate, [2, 2])
}
pub(crate) fn accept_resolution(
    listener: &TcpListener,
    rate: u32,
    capture_resolution: [u32; 2],
) -> WebSocket<TcpStream> {
    let mut socket = hello(listener);
    socket
        .send(Message::binary(
            ControlMessage::CaptureSettings {
                refresh_rate_hz: rate,
                capture_resolution,
            }
            .encode()
            .unwrap(),
        ))
        .unwrap();
    socket
}
fn connect(address: SocketAddr) -> (Sender, CaptureOptions) {
    let (mut sender, msg) = Sender::connect(&format!("http://{address}").parse().unwrap()).unwrap();
    let settings = sender.wait_for_capture(msg, || Ok(true)).unwrap().unwrap();
    (sender, settings)
}
fn send(
    sender: &mut Sender,
    encoder: &mut Encoder,
    header: Header,
    pixels: &[u8],
    content: Content,
) -> Result<bool> {
    let Some(frame) = encoder.encode(header, pixels, content)? else {
        return Ok(false);
    };
    sender.send(frame, content)?;
    Ok(true)
}

#[test]
fn receiver_close_is_visible_without_sending_frames() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let receiver = thread::spawn(move || accept(&listener, 30));
    let (mut sender, _) = connect(address);
    sender.check_connection().unwrap();
    receiver
        .join()
        .unwrap()
        .get_ref()
        .shutdown(std::net::Shutdown::Both)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    while sender.check_connection().is_ok() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn loopback_raw_and_compressed_frames_have_exact_boundaries() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let receiver = thread::spawn(move || {
        let mut socket = accept(&listener, 30);
        for compressed in [false, true] {
            let bytes = socket.read().unwrap().into_data();
            let header = Header::decode(bytes[..HEADER_LEN].try_into().unwrap()).unwrap();
            assert_eq!(header.codec == prysm_webos_protocol::Codec::Lz4, compressed);
            let payload = &bytes[HEADER_LEN..];
            assert_eq!(payload.len(), header.payload_len);
            if compressed {
                let mut decoded = vec![0; header.raw_len().unwrap()];
                decompress(payload, &mut decoded).unwrap();
                assert_eq!(decoded, vec![42; 600]);
            } else {
                assert_eq!(payload, vec![42; 600]);
            }
        }
    });
    let (mut sender, settings) = connect(address);
    let header = Header {
        width: 20,
        height: 10,
        format: PixelFormat::Rgb,
        codec: prysm_webos_protocol::Codec::Raw,
        payload_len: 600,
    };
    for compression in [Compression::None, Compression::Lz4] {
        let mut encoder = Encoder::new(compression, settings.fps, H264_BITRATE).unwrap();
        assert!(
            send(
                &mut sender,
                &mut encoder,
                header,
                &[42; 600],
                Content::Video
            )
            .unwrap()
        );
    }
    receiver.join().unwrap();
}

#[test]
fn h264_rate_control_skips_without_packets_or_reconnects() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let receiver = thread::spawn(move || {
        let mut socket = accept(&listener, 60);
        let mut decoder = prysm_webos_protocol::h264::Decoder::new().unwrap();
        let mut received = 0;
        loop {
            let Ok(msg) = socket.read() else {
                return received;
            };
            let bytes = msg.into_data();
            let header = Header::decode(bytes[..HEADER_LEN].try_into().unwrap()).unwrap();
            let payload = &bytes[HEADER_LEN..];
            assert_eq!(payload.len(), header.payload_len);
            decoder.decode(header, payload).unwrap();
            received += 1;
        }
    });
    let (mut sender, settings) = connect(address);
    assert_eq!(settings.fps, 60);
    let mut encoder = Encoder::new(Compression::H264Software, settings.fps, 64_000).unwrap();
    let header = Header {
        width: 320,
        height: 180,
        format: PixelFormat::Rgb,
        codec: prysm_webos_protocol::Codec::Raw,
        payload_len: 320 * 180 * 3,
    };
    let mut pixels = vec![0; header.payload_len];
    let mut random = 1_u32;
    let mut sent = 0;
    let mut skipped = 0;
    for _ in 0..120 {
        for byte in &mut pixels {
            random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *byte = random.to_le_bytes()[3];
        }
        if send(&mut sender, &mut encoder, header, &pixels, Content::Video).unwrap() {
            sent += 1;
        } else {
            skipped += 1;
        }
        sender.check_connection().unwrap();
    }
    drop(sender);
    assert!(skipped > 0);
    assert_eq!(receiver.join().unwrap(), sent);
}

#[test]
fn new_connections_start_with_an_independently_decodable_frame() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let receiver = thread::spawn(move || {
        for (rate, resolution) in [(30, [64, 32]), (15, [32, 16])] {
            let mut socket = accept_resolution(&listener, rate, resolution);
            let bytes = socket.read().unwrap().into_data();
            let header = Header::decode(bytes[..HEADER_LEN].try_into().unwrap()).unwrap();
            assert_eq!(header.codec, prysm_webos_protocol::Codec::H264);
            let payload = &bytes[HEADER_LEN..];
            assert_eq!(payload.len(), header.payload_len);
            let mut decoder = prysm_webos_protocol::h264::Decoder::new().unwrap();
            assert_eq!([header.width, header.height], resolution);
            assert_eq!(
                decoder.decode(header, payload).unwrap().len(),
                (resolution[0] * resolution[1] * 3) as usize
            );
        }
    });
    for resolution in [[64, 32], [32, 16]] {
        let (mut sender, settings) = connect(address);
        assert_eq!([settings.width, settings.height], resolution);
        let mut encoder =
            Encoder::new(Compression::H264Software, settings.fps, H264_BITRATE).unwrap();
        let header = Header {
            width: resolution[0],
            height: resolution[1],
            format: PixelFormat::Rgb,
            codec: prysm_webos_protocol::Codec::Raw,
            payload_len: (resolution[0] * resolution[1] * 3) as usize,
        };
        assert!(
            send(
                &mut sender,
                &mut encoder,
                header,
                &[255, 0, 0].repeat((resolution[0] * resolution[1]) as usize),
                Content::Blended
            )
            .unwrap()
        );
    }
    receiver.join().unwrap();
}

#[test]
fn failed_connection_cannot_create_a_sender_or_queue_frames() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    assert!(Sender::connect(&format!("http://{address}").parse().unwrap()).is_err());
    let listener = TcpListener::bind(address).unwrap();
    let receiver = thread::spawn(move || {
        let mut socket = accept(&listener, 15);
        let bytes = socket.read().unwrap().into_data();
        assert_eq!(&bytes[HEADER_LEN..], &[1, 2, 3]);
    });
    let (mut sender, settings) = connect(address);
    assert_eq!(settings.fps, 15);
    let mut encoder = Encoder::new(Compression::None, settings.fps, H264_BITRATE).unwrap();
    let header = Header {
        width: 1,
        height: 1,
        format: PixelFormat::Rgb,
        codec: prysm_webos_protocol::Codec::Raw,
        payload_len: 3,
    };
    assert!(
        send(
            &mut sender,
            &mut encoder,
            header,
            &[1, 2, 3],
            Content::Video
        )
        .unwrap()
    );
    receiver.join().unwrap();
}

#[test]
fn failed_and_stalled_handshakes_report_distinct_connection_outcomes() {
    use std::io::{Read, Write};
    let exporter = prysm_metrics::start(
        "127.0.0.1:0".parse().unwrap(),
        prysm_metrics::Role::Capturer,
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = format!("http://{}", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let receiver =
        thread::spawn(move || hello(&listener).send(Message::binary("not Prysm")).unwrap());
    assert!(Sender::connect(&addr).is_err());
    receiver.join().unwrap();
    partial_reply_times_out(false);

    let mut client = TcpStream::connect(exporter.address()).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    client.write_all(b"GET /metrics HTTP/1.0\r\n\r\n").unwrap();
    let mut metrics = String::new();
    client.read_to_string(&mut metrics).unwrap();
    for outcome in ["Failed", "TimedOut"] {
        assert!(metrics.contains(&format!(
            r#"component="FrameSender",operation="Connect",outcome="{outcome}""#
        )));
    }
}

#[test]
fn stalled_receiver_cannot_extend_the_frame_deadline() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || accept(&listener, 30));
    let (mut sender, _) = connect(address);
    let peer = server.join().unwrap();
    nix::sys::socket::setsockopt(
        sender.socket.get_ref().get_ref(),
        nix::sys::socket::sockopt::SndBuf,
        &4096,
    )
    .unwrap();
    nix::sys::socket::setsockopt(peer.get_ref(), nix::sys::socket::sockopt::RcvBuf, &4096).unwrap();
    let header = Header {
        width: 1920,
        height: 1080,
        format: PixelFormat::Rgb,
        codec: prysm_webos_protocol::Codec::Raw,
        payload_len: 1920 * 1080 * 3,
    };
    let pixels = vec![64; header.payload_len];
    let mut encoder = Encoder::new(Compression::None, 30, H264_BITRATE).unwrap();
    let start = Instant::now();
    assert!(send(&mut sender, &mut encoder, header, &pixels, Content::Video).is_err());
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn https_ipv6_literal_reaches_the_tls_peer() {
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use std::os::fd::AsFd;

    let listener = TcpListener::bind("[::1]:0").unwrap();
    let instance = format!("https://{}", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let peer = thread::spawn(move || {
        let mut waiting = [PollFd::new(listener.as_fd(), PollFlags::POLLIN)];
        let timeout = PollTimeout::try_from(CONTROL_TIMEOUT).unwrap();
        if poll(&mut waiting, timeout).unwrap() == 0 {
            return false;
        }
        let (mut tcp, _) = listener.accept().unwrap();
        tcp.set_read_timeout(Some(CONTROL_TIMEOUT)).unwrap();
        let mut acceptor = rustls::server::Acceptor::default();
        while acceptor.read_tls(&mut tcp).unwrap() != 0 {
            if acceptor.accept().unwrap().is_some() {
                return true;
            }
        }
        false
    });
    // The peer closes after ClientHello without presenting a certificate.
    assert!(Sender::connect(&instance).is_err());
    assert!(peer.join().unwrap(), "IPv6 was rejected before TLS");
}

#[test]
fn idle_renewals_keep_a_healthy_tv_waiting_until_capture_starts() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let peer = thread::spawn(move || {
        let mut socket = hello(&listener);
        let start = Instant::now();
        while start.elapsed() <= CONTROL_TIMEOUT {
            socket
                .send(Message::binary(ControlMessage::Idle.encode().unwrap()))
                .unwrap();
            thread::sleep(prysm_webos_protocol::IDLE_RENEWAL);
        }
        socket
            .send(Message::binary(
                ControlMessage::CaptureSettings {
                    refresh_rate_hz: 15,
                    capture_resolution: [2, 2],
                }
                .encode()
                .unwrap(),
            ))
            .unwrap();
    });
    let (_, settings) = connect(address);
    assert_eq!(settings.fps, 15);
    peer.join().unwrap();
}

#[test]
fn trickling_settings_after_idle_cannot_renew_the_deadline() {
    partial_reply_times_out(true);
}

fn partial_reply_times_out(after_idle: bool) {
    use std::io::Write;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let instance = format!("http://{}", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let peer = thread::spawn(move || {
        let mut socket = hello(&listener);
        if after_idle {
            socket
                .send(Message::binary(ControlMessage::Idle.encode().unwrap()))
                .unwrap();
        }
        let payload = ControlMessage::CaptureSettings {
            refresh_rate_hz: 30,
            capture_resolution: [2, 2],
        }
        .encode()
        .unwrap();
        let tcp = socket.get_mut();
        tcp.write_all(&[0x82, u8::try_from(payload.len()).unwrap()])
            .unwrap();
        thread::sleep(CONTROL_TIMEOUT * 3 / 5);
        tcp.write_all(&payload[..1]).unwrap();
        thread::sleep(CONTROL_TIMEOUT * 3 / 5);
        let _ = tcp.write_all(&payload[1..]);
    });
    let result = Sender::connect(&instance)
        .and_then(|(mut sender, msg)| sender.wait_for_capture(msg, || Ok(true)));
    let err = result.expect_err("peer extended the deadline by trickling bytes");
    assert!(
        err.downcast_ref::<io::Error>()
            .is_some_and(|e| e.kind() == io::ErrorKind::TimedOut),
        "{err:#}"
    );
    peer.join().unwrap();
}
