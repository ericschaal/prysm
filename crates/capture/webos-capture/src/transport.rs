//! One negotiated TCP connection; encoding and capture policy belong to Session.
use crate::{config::CaptureOptions, encoder::EncodedFrame};
use anyhow::{Context, Result};
use nix::{
    errno::Errno,
    poll::{PollFd, PollFlags, poll},
};
use prysm_metrics::{
    BytesKind, Codec, Content, EncoderEvent, FrameSenderEvent, Stage, WireBoundary,
};
use prysm_webos_protocol::{ControlMessage, HELLO};
use std::{
    io::{self, IoSlice, Read, Write},
    net::{SocketAddr, TcpStream},
    os::fd::AsFd,
    time::{Duration, Instant},
};

const DEADLINE: Duration = Duration::from_millis(100);

pub struct Sender {
    destination: SocketAddr,
    socket: TcpStream,
}

impl Sender {
    /// Establish one worker connection and validate its initial control message.
    pub fn connect(destination: SocketAddr) -> Result<(Self, ControlMessage)> {
        let result = (|| {
            prysm_metrics::event(FrameSenderEvent::ConnectAttempt);
            let connect_time = prysm_metrics::timer(Stage::FrameConnect);
            let mut socket =
                TcpStream::connect_timeout(&destination, DEADLINE).inspect_err(|error| {
                    prysm_metrics::event(if error.kind() == io::ErrorKind::TimedOut {
                        FrameSenderEvent::ConnectTimedOut
                    } else {
                        FrameSenderEvent::ConnectFailed
                    });
                })?;
            drop(connect_time);
            socket
                .set_nodelay(true)
                .and_then(|()| socket.set_nonblocking(true))
                .inspect_err(|_| {
                    prysm_metrics::event(FrameSenderEvent::ConfigureFailed);
                })?;
            write_frame(&mut socket, &HELLO.encode()?, &[], DEADLINE)
                .context("cannot send webOS hello")?;
            let message =
                read_settings(&mut socket).context("cannot read webOS capture settings")?;
            anyhow::ensure!(
                matches!(
                    message,
                    ControlMessage::Idle | ControlMessage::CaptureSettings { .. }
                ),
                "expected webOS capture settings or idle"
            );
            prysm_metrics::event(FrameSenderEvent::Connected);
            anyhow::Ok((
                Self {
                    destination,
                    socket,
                },
                message,
            ))
        })();
        result.with_context(|| format!("negotiating capture with {destination}"))
    }

    pub fn check_connection(&self) -> Result<()> {
        match self.socket.peek(&mut [0]) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(()),
            Err(error) => Err(error.into()),
            Ok(_) => anyhow::bail!("receiver closed or sent unexpected control data"),
        }
    }

    /// Wait without opening native resources; partial messages retain their deadline.
    pub fn wait_for_capture(
        &mut self,
        mut message: ControlMessage,
        stop: &std::sync::atomic::AtomicBool,
    ) -> Result<Option<CaptureOptions>> {
        loop {
            match message {
                ControlMessage::CaptureSettings {
                    refresh_rate_hz,
                    capture_resolution,
                } => {
                    return Ok(Some(CaptureOptions {
                        fps: u8::try_from(refresh_rate_hz)?,
                        width: capture_resolution[0],
                        height: capture_resolution[1],
                    }));
                }
                ControlMessage::Idle => (),
                ControlMessage::Hello { .. } => anyhow::bail!("expected webOS capture settings"),
            }
            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                return Ok(None);
            }
            match self.socket.peek(&mut [0]) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(error) => return Err(error.into()),
                Ok(0) => anyhow::bail!("receiver disconnected"),
                Ok(_) => message = read_settings(&mut self.socket)?,
            }
        }
    }

    pub fn send(&mut self, frame: EncodedFrame<'_>, content: Content) -> Result<()> {
        let result = (|| {
            let header = frame.header;
            let bytes = header.encode().inspect_err(|_| {
                prysm_metrics::event(EncoderEvent::HeaderRejected);
            })?;
            let _write_time = prysm_metrics::timer(Stage::FrameWrite);
            write_frame(&mut self.socket, &bytes, frame.pixels, DEADLINE)
                .inspect_err(|error| record_write_error(error.kind()))
                .with_context(|| format!("sending capture to {}", self.destination))?;
            prysm_metrics::wire_frame(
                WireBoundary::SocketAccepted,
                header.format.into(),
                Codec::from(&header),
                content,
                header.width,
                header.height,
            );
            anyhow::Ok(())
        })();
        if result.is_err() {
            self.disconnect();
        }
        result
    }

    pub fn disconnect(&self) {
        // Close the peer before potentially blocking LG teardown, even while Sender is retained.
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
    }
}

fn read_settings(socket: &mut TcpStream) -> Result<ControlMessage> {
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut prefix = [0; 4];
    read_control_bytes(socket, &mut prefix, deadline)?;
    let length = ControlMessage::decode_length(prefix)?;
    let mut payload = vec![0; length];
    read_control_bytes(socket, &mut payload, deadline)?;
    ControlMessage::decode(&payload)
}

fn read_control_bytes(
    socket: &mut TcpStream,
    mut bytes: &mut [u8],
    deadline: Instant,
) -> io::Result<()> {
    while !bytes.is_empty() {
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        match socket.read(bytes) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(read) => bytes = &mut bytes[read..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let mut fds = [PollFd::new(socket.as_fd(), PollFlags::POLLIN)];
                let millis = u16::try_from(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .as_millis()
                        + 1,
                )
                .unwrap_or(u16::MAX);
                match poll(&mut fds, millis) {
                    Ok(_) | Err(Errno::EINTR) => {}
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn record_write_error(kind: io::ErrorKind) {
    prysm_metrics::event(if kind == io::ErrorKind::TimedOut {
        FrameSenderEvent::FrameWriteTimedOut
    } else {
        FrameSenderEvent::FrameWriteFailed
    });
}

fn write_frame(
    socket: &mut TcpStream,
    header: &[u8],
    payload: &[u8],
    timeout: Duration,
) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    let mut slices = [IoSlice::new(header), IoSlice::new(payload)];
    let mut remaining = &mut slices[..];
    while !remaining.is_empty() {
        let now = Instant::now();
        if now >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        match socket.write_vectored(remaining) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(written) => {
                prysm_metrics::bytes(BytesKind::SenderSocketAccepted, written);
                IoSlice::advance_slices(&mut remaining, written);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let mut fds = [PollFd::new(socket.as_fd(), PollFlags::POLLOUT)];
                let millis = u16::try_from(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .as_millis()
                        + 1,
                )
                .unwrap_or(u16::MAX);
                let _blocked_time = prysm_metrics::timer(Stage::FrameWriteWait);
                match poll(&mut fds, millis) {
                    Ok(_) | Err(Errno::EINTR) => {}
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::encoder::{Encoder, H264_BITRATE};
    use prysm_webos_protocol::{Compression, HEADER_LEN, Header, PixelFormat, decompress};
    use std::{io::Read, net::TcpListener, sync::atomic::AtomicBool, thread};
    pub(crate) fn accept(listener: &TcpListener, rate: u32) -> TcpStream {
        accept_resolution(listener, rate, [2, 2])
    }

    pub(crate) fn accept_resolution(
        listener: &TcpListener,
        rate: u32,
        capture_resolution: [u32; 2],
    ) -> TcpStream {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        read_hello(&mut socket);
        for byte in (ControlMessage::CaptureSettings {
            refresh_rate_hz: rate,
            capture_resolution,
        })
        .encode()
        .unwrap()
        {
            socket.write_all(&[byte]).unwrap();
        }
        socket
    }

    fn read_hello(socket: &mut TcpStream) {
        let mut prefix = [0; 4];
        socket.read_exact(&mut prefix).unwrap();
        let mut payload = vec![0; ControlMessage::decode_length(prefix).unwrap()];
        socket.read_exact(&mut payload).unwrap();
        assert_eq!(ControlMessage::decode(&payload).unwrap(), HELLO);
    }

    fn connect(address: SocketAddr) -> (Sender, CaptureOptions) {
        let (mut sender, message) = Sender::connect(address).unwrap();
        let settings = sender
            .wait_for_capture(message, &AtomicBool::new(false))
            .unwrap()
            .unwrap();
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
        let (sender, _) = connect(address);
        sender.check_connection().unwrap();
        receiver
            .join()
            .unwrap()
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
                let mut bytes = [0; HEADER_LEN];
                socket.read_exact(&mut bytes).unwrap();
                let header = Header::decode(&bytes).unwrap();
                assert_eq!(header.codec == prysm_webos_protocol::Codec::Lz4, compressed);
                let mut payload = vec![0; header.payload_len];
                socket.read_exact(&mut payload).unwrap();
                if compressed {
                    let mut decoded = vec![0; header.raw_len().unwrap()];
                    decompress(&payload, &mut decoded).unwrap();
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
                let mut bytes = [0; HEADER_LEN];
                if let Err(error) = socket.read_exact(&mut bytes) {
                    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
                    return received;
                }
                let header = Header::decode(&bytes).unwrap();
                let mut payload = vec![0; header.payload_len];
                socket.read_exact(&mut payload).unwrap();
                decoder.decode(header, &payload).unwrap();
                received += 1;
            }
        });
        let (mut sender, settings) = connect(address);
        assert_eq!(settings.fps, 60);
        let mut encoder = Encoder::new(Compression::H264, settings.fps, 64_000).unwrap();
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
                let mut bytes = [0; HEADER_LEN];
                socket.read_exact(&mut bytes).unwrap();
                let header = Header::decode(&bytes).unwrap();
                assert_eq!(header.codec, prysm_webos_protocol::Codec::H264);
                let mut payload = vec![0; header.payload_len];
                socket.read_exact(&mut payload).unwrap();
                let mut decoder = prysm_webos_protocol::h264::Decoder::new().unwrap();
                assert_eq!([header.width, header.height], resolution);
                assert_eq!(
                    decoder.decode(header, &payload).unwrap().len(),
                    (resolution[0] * resolution[1] * 3) as usize
                );
            }
        });
        for resolution in [[64, 32], [32, 16]] {
            let (mut sender, settings) = connect(address);
            assert_eq!([settings.width, settings.height], resolution);
            let mut encoder = Encoder::new(Compression::H264, settings.fps, H264_BITRATE).unwrap();
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
        assert!(Sender::connect(address).is_err());
        let listener = TcpListener::bind(address).unwrap();
        let receiver = thread::spawn(move || {
            let mut socket = accept(&listener, 15);
            let mut bytes = [0; HEADER_LEN + 3];
            socket.read_exact(&mut bytes).unwrap();
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
    fn telemetry_attributes_connect_encoding_and_write_timeout() {
        let exporter = prysm_metrics::start(
            "127.0.0.1:0".parse().unwrap(),
            prysm_metrics::Role::Capturer,
        )
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let destination = listener.local_addr().unwrap();
        drop(listener);
        assert!(Sender::connect(destination).is_err());
        let mut encoder = Encoder::new(Compression::None, 30, H264_BITRATE).unwrap();
        let header = Header {
            width: 1,
            height: 1,
            format: PixelFormat::Rgb,
            codec: prysm_webos_protocol::Codec::Raw,
            payload_len: 3,
        };
        assert!(encoder.encode(header, &[0; 2], Content::Video).is_err());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut receiver, _) = listener.accept().unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        socket.set_nonblocking(true).unwrap();
        nix::sys::socket::setsockopt(&socket, nix::sys::socket::sockopt::SndBuf, &4096).unwrap();
        let mut sender = Sender {
            destination,
            socket,
        };
        let header = Header {
            width: 1920,
            height: 1080,
            format: PixelFormat::Rgb,
            codec: prysm_webos_protocol::Codec::Raw,
            payload_len: 6_220_800,
        };
        assert!(
            send(
                &mut sender,
                &mut encoder,
                header,
                &vec![0; 6_220_800],
                Content::Video
            )
            .is_err()
        );
        // The peer must see EOF even while the owner retains Sender for native teardown.
        let mut discarded = Vec::new();
        receiver.read_to_end(&mut discarded).unwrap();
        let mut socket = TcpStream::connect(exporter.address()).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        socket.write_all(b"GET /metrics HTTP/1.0\r\n\r\n").unwrap();
        let mut text = String::new();
        socket.read_to_string(&mut text).unwrap();
        for labels in [
            "component=\"FrameSender\",operation=\"Connect\",outcome=\"Failed\"",
            "component=\"FrameEncoder\",operation=\"ValidateInput\",outcome=\"Rejected\"",
            "component=\"FrameSender\",operation=\"WriteFrame\",outcome=\"TimedOut\"",
        ] {
            assert!(text.contains(labels), "missing {labels}: {text}");
        }
        assert!(text.contains("codec=\"None\",content=\"Video\",width=\"1920\",height=\"1080\""));
    }

    #[test]
    fn rejects_invalid_or_truncated_settings() {
        for json in [
            r#"{"type":"hello","version":1}"#,
            r#"{"type":"capture_settings","refresh_rate_hz":30,"capture_resolution":[3,360]}"#,
            r#"{"type":"capture_settings","refresh_rate_hz":0,"capture_resolution":[640,360]}"#,
            r#"{"type":"capture_settings","refresh_rate_hz":61,"capture_resolution":[640,360]}"#,
        ] {
            let mut bytes = u32::try_from(json.len()).unwrap().to_be_bytes().to_vec();
            bytes.extend_from_slice(json.as_bytes());
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let receiver = thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                read_hello(&mut socket);
                socket.write_all(&bytes).unwrap();
            });
            assert!(Sender::connect(address).is_err());
            receiver.join().unwrap();
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let receiver = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            read_hello(&mut socket);
            socket.write_all(&30u32.to_be_bytes()).unwrap();
            socket.write_all(b"{").unwrap();
        });
        assert!(Sender::connect(address).is_err());
        receiver.join().unwrap();
    }

    #[test]
    fn stalled_handshake_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let receiver = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            read_hello(&mut socket);
            assert_eq!(socket.read(&mut [0; 1]).unwrap(), 0);
        });
        let start = Instant::now();
        let error = Sender::connect(address).err().unwrap();
        assert_eq!(
            error.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::TimedOut
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        receiver.join().unwrap();
    }

    #[test]
    fn stalled_receiver_cannot_extend_the_frame_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_receiver, _) = listener.accept().unwrap();
        socket.set_nonblocking(true).unwrap();
        nix::sys::socket::setsockopt(&socket, nix::sys::socket::sockopt::SndBuf, &4096).unwrap();
        let start = Instant::now();
        let error = write_frame(
            &mut socket,
            &[0; 24],
            &vec![0; 6_220_800],
            Duration::from_millis(30),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
