use crate::cli::Compression;
use anyhow::{Context, Result};
use nix::{
    errno::Errno,
    poll::{PollFd, PollFlags, poll},
};
use prysm_metrics::{
    BytesKind, Codec, Content, EncoderEvent, FrameSenderEvent, Stage, WireBoundary,
};
use prysm_webos_protocol::{ControlMessage, HELLO, Header, compress};
use std::{
    io::{self, IoSlice, Read, Write},
    net::{SocketAddr, TcpStream},
    os::fd::AsFd,
    time::{Duration, Instant},
};

const DEADLINE: Duration = Duration::from_millis(100);

#[derive(Debug, PartialEq, Eq)]
pub enum SendOutcome {
    Sent,
    Backoff,
    EncoderSkipped,
}

pub struct Sender {
    destination: SocketAddr,
    socket: Option<TcpStream>,
    retry_after: Instant,
    compression: Compression,
    scratch: Vec<u8>,
    has_connected: bool,
    encoder: Option<prysm_webos_protocol::h264::Encoder>,
    pub h264_bitrate: u32,
    pub fps: u8,
    pub capture_resolution: [u32; 2],
}

impl Sender {
    pub fn new(destination: SocketAddr, compression: Compression) -> Self {
        Self {
            destination,
            socket: None,
            retry_after: Instant::now(),
            compression,
            scratch: Vec::new(),
            has_connected: false,
            encoder: None,
            h264_bitrate: 2_000_000,
            fps: 30,
            capture_resolution: [320, 180],
        }
    }

    pub fn connected(&self) -> bool {
        self.socket.is_some()
    }

    pub fn ready(&self) -> bool {
        self.socket.is_some() || Instant::now() >= self.retry_after
    }

    /// Negotiate capture settings before acquiring or encoding any frames.
    /// Returns false while reconnect backoff is active.
    pub fn connect(&mut self) -> Result<bool> {
        if self.socket.is_some() {
            return Ok(true);
        }
        if !self.ready() {
            return Ok(false);
        }
        let result: Result<()> = (|| {
            prysm_metrics::event(FrameSenderEvent::ConnectAttempt);
            let connect_time = prysm_metrics::timer(Stage::FrameConnect);
            let mut connected = TcpStream::connect_timeout(&self.destination, DEADLINE)
                .inspect_err(|error| {
                    prysm_metrics::event(if error.kind() == io::ErrorKind::TimedOut {
                        FrameSenderEvent::ConnectTimedOut
                    } else {
                        FrameSenderEvent::ConnectFailed
                    });
                })?;
            drop(connect_time);
            connected
                .set_nodelay(true)
                .and_then(|()| connected.set_nonblocking(true))
                .inspect_err(|_| prysm_metrics::event(FrameSenderEvent::ConfigureFailed))?;
            write_frame(&mut connected, &HELLO.encode()?, &[], DEADLINE)
                .context("cannot send webOS hello")?;
            let message =
                read_settings(&mut connected).context("cannot read webOS capture settings")?;
            let ControlMessage::CaptureSettings {
                refresh_rate_hz,
                capture_resolution,
            } = message
            else {
                anyhow::bail!("expected webOS capture settings");
            };
            self.fps = u8::try_from(refresh_rate_hz)?;
            self.capture_resolution = capture_resolution;
            self.socket = Some(connected);
            prysm_metrics::event(if self.has_connected {
                FrameSenderEvent::Reconnected
            } else {
                FrameSenderEvent::Connected
            });
            self.has_connected = true;
            Ok(())
        })();
        if result.is_err() {
            self.retry_after = Instant::now() + Duration::from_secs(1);
        }
        result.with_context(|| format!("negotiating capture with {}", self.destination))?;
        Ok(true)
    }

    /// Send on the negotiated connection, without a frame queue or implicit reconnect.
    pub fn send(
        &mut self,
        mut header: Header,
        pixels: &[u8],
        content: Content,
    ) -> Result<SendOutcome> {
        let raw_len = header
            .raw_len()
            .inspect_err(|_| prysm_metrics::event(EncoderEvent::InputRejected))?;
        if raw_len != pixels.len() {
            prysm_metrics::event(EncoderEvent::InputRejected);
            anyhow::bail!("incorrect sender frame length");
        }
        if self.socket.is_none() {
            prysm_metrics::event(FrameSenderEvent::BackoffSkipped);
            return Ok(SendOutcome::Backoff);
        }
        let result: Result<SendOutcome> = (|| {
            header.codec = prysm_webos_protocol::Codec::Raw;
            let encode_time = prysm_metrics::timer(Stage::FrameEncode);
            let payload = if self.compression == Compression::H264 {
                if self.encoder.is_none() {
                    self.encoder = Some(
                        prysm_webos_protocol::h264::Encoder::new(self.h264_bitrate, self.fps)
                            .inspect_err(|_| {
                                prysm_metrics::event(EncoderEvent::InitializationFailed)
                            })?,
                    );
                }
                let kind = self
                    .encoder
                    .as_mut()
                    .expect("initialized encoder")
                    .encode(header, pixels, &mut self.scratch)
                    .inspect_err(|_| prysm_metrics::event(EncoderEvent::H264EncodeFailed))?;
                let Some(kind) = kind else {
                    prysm_metrics::event(EncoderEvent::H264RateControlSkipped);
                    return Ok(SendOutcome::EncoderSkipped);
                };
                prysm_metrics::event(match kind {
                    prysm_webos_protocol::h264::FrameKind::Idr => EncoderEvent::H264IdrEncoded,
                    prysm_webos_protocol::h264::FrameKind::Intra => {
                        EncoderEvent::H264IntraFrameEncoded
                    }
                    prysm_webos_protocol::h264::FrameKind::Predicted => {
                        EncoderEvent::H264PredictedFrameEncoded
                    }
                });
                header.codec = prysm_webos_protocol::Codec::H264;
                header.format = prysm_webos_protocol::PixelFormat::Rgb;
                self.scratch.as_slice()
            } else if self.compression == Compression::Lz4 {
                if let Some(length) = compress(pixels, &mut self.scratch)
                    .inspect_err(|_| prysm_metrics::event(EncoderEvent::CompressionFailed))?
                {
                    header.codec = prysm_webos_protocol::Codec::Lz4;
                    &self.scratch[..length]
                } else {
                    prysm_metrics::event(EncoderEvent::RawFallback);
                    pixels
                }
            } else {
                pixels
            };
            drop(encode_time);
            header.payload_len = payload.len();
            let bytes = header
                .encode()
                .inspect_err(|_| prysm_metrics::event(EncoderEvent::HeaderRejected))?;
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
            let _write_time = prysm_metrics::timer(Stage::FrameWrite);
            write_frame(
                self.socket.as_mut().expect("connected socket"),
                &bytes,
                payload,
                DEADLINE,
            )
            .inspect_err(|error| record_write_error(error.kind()))?;
            prysm_metrics::wire_frame(
                WireBoundary::SocketAccepted,
                header.format.into(),
                Codec::from(&header),
                content,
                header.width,
                header.height,
            );
            Ok(SendOutcome::Sent)
        })();
        if result.is_err() {
            // A partially written frame cannot be resumed on a later connection.
            self.socket = None;
            self.encoder = None;
            self.retry_after = Instant::now() + Duration::from_secs(1);
        }
        result.with_context(|| format!("sending capture to {}", self.destination))
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
    use prysm_webos_protocol::{HEADER_LEN, PixelFormat, decompress};
    use std::{io::Read, net::TcpListener, thread};

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
        let mut sender = Sender::new(address, Compression::None);
        let header = Header {
            width: 20,
            height: 10,
            format: PixelFormat::Rgb,
            codec: prysm_webos_protocol::Codec::Raw,
            payload_len: 600,
        };
        assert!(sender.connect().unwrap());
        assert_eq!(
            sender.send(header, &[42; 600], Content::Video).unwrap(),
            SendOutcome::Sent
        );
        sender.compression = Compression::Lz4;
        assert_eq!(
            sender.send(header, &[42; 600], Content::Video).unwrap(),
            SendOutcome::Sent
        );
        receiver.join().unwrap();
    }

    #[test]
    fn h264_rate_control_skips_without_packets_or_reconnects() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut sender = Sender::new(listener.local_addr().unwrap(), Compression::H264);
        sender.h264_bitrate = 64_000;
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
        assert!(sender.connect().unwrap());
        assert_eq!(sender.fps, 60);
        for _ in 0..120 {
            for byte in &mut pixels {
                random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *byte = random.to_le_bytes()[3];
            }
            match sender.send(header, &pixels, Content::Video).unwrap() {
                SendOutcome::Sent => sent += 1,
                SendOutcome::EncoderSkipped => skipped += 1,
                SendOutcome::Backoff => panic!("rate control must not disconnect"),
            }
            assert!(sender.socket.is_some());
            assert!(sender.encoder.is_some());
        }
        drop(sender);
        assert!(skipped > 0);
        assert_eq!(receiver.join().unwrap(), sent);
    }

    #[test]
    fn h264_reconnect_starts_with_an_independently_decodable_frame() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut sender = Sender::new(listener.local_addr().unwrap(), Compression::H264);
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
        let header = Header {
            width: 64,
            height: 32,
            format: PixelFormat::Rgb,
            codec: prysm_webos_protocol::Codec::Raw,
            payload_len: 64 * 32 * 3,
        };
        let pixels = [255, 0, 0].repeat(64 * 32);
        assert!(sender.connect().unwrap());
        assert_eq!(
            sender.send(header, &pixels, Content::Blended).unwrap(),
            SendOutcome::Sent
        );
        sender
            .socket
            .as_ref()
            .unwrap()
            .shutdown(std::net::Shutdown::Write)
            .unwrap();
        assert!(sender.send(header, &pixels, Content::Blended).is_err());
        assert!(sender.encoder.is_none());
        sender.retry_after = Instant::now();
        assert!(sender.connect().unwrap());
        assert_eq!(sender.capture_resolution, [32, 16]);
        let header = Header {
            width: 32,
            height: 16,
            payload_len: 32 * 16 * 3,
            ..header
        };
        let pixels = [255, 0, 0].repeat(32 * 16);
        assert_eq!(
            sender.send(header, &pixels, Content::Blended).unwrap(),
            SendOutcome::Sent
        );
        assert_eq!(sender.fps, 15);
        receiver.join().unwrap();
    }

    #[test]
    fn refuses_connection_then_backs_off_without_queuing() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let mut sender = Sender::new(address, Compression::None);
        let header = Header {
            width: 1,
            height: 1,
            format: PixelFormat::Rgb,
            codec: prysm_webos_protocol::Codec::Raw,
            payload_len: 3,
        };
        assert!(sender.connect().is_err());
        assert!(!sender.ready());
        assert_eq!(
            sender.send(header, &[0; 3], Content::Video).unwrap(),
            SendOutcome::Backoff
        );
        assert!(sender.socket.is_none());
        let listener = TcpListener::bind(address).unwrap();
        sender.retry_after = Instant::now();
        assert!(sender.ready());
        let receiver = thread::spawn(move || {
            let mut socket = accept(&listener, 15);
            let mut bytes = [0; HEADER_LEN + 3];
            socket.read_exact(&mut bytes).unwrap();
            assert_eq!(&bytes[HEADER_LEN..], &[1, 2, 3]);
        });
        assert!(sender.connect().unwrap());
        assert_eq!(sender.fps, 15);
        assert_eq!(
            sender.send(header, &[1, 2, 3], Content::Video).unwrap(),
            SendOutcome::Sent
        );
        receiver.join().unwrap();
    }

    #[test]
    fn telemetry_attributes_connect_encoding_backoff_and_write_timeout() {
        // One opt-in exporter for this test process. Other transport tests may
        // add samples concurrently, so assertions test distinct label series.
        let exporter = prysm_metrics::start(
            "127.0.0.1:0".parse().unwrap(),
            prysm_metrics::Role::Capturer,
        )
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let destination = listener.local_addr().unwrap();
        drop(listener);
        let mut sender = Sender::new(destination, Compression::None);
        let header = Header {
            width: 1,
            height: 1,
            format: PixelFormat::Rgb,
            codec: prysm_webos_protocol::Codec::Raw,
            payload_len: 3,
        };
        assert!(sender.connect().is_err());
        assert_eq!(
            sender.send(header, &[0; 3], Content::Video).unwrap(),
            SendOutcome::Backoff
        );
        assert!(sender.send(header, &[0; 2], Content::Video).is_err());

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_receiver, _) = listener.accept().unwrap();
        socket.set_nonblocking(true).unwrap();
        nix::sys::socket::setsockopt(&socket, nix::sys::socket::sockopt::SndBuf, &4096).unwrap();
        sender.socket = Some(socket);
        let header = Header {
            width: 1920,
            height: 1080,
            format: PixelFormat::Rgb,
            codec: prysm_webos_protocol::Codec::Raw,
            payload_len: 6_220_800,
        };
        assert!(
            sender
                .send(header, &vec![0; 6_220_800], Content::Video)
                .is_err()
        );
        assert!(sender.socket.is_none());
        assert!(!sender.ready());

        let mut socket = TcpStream::connect(exporter.address()).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        socket.write_all(b"GET /metrics HTTP/1.0\r\n\r\n").unwrap();
        let mut text = String::new();
        socket.read_to_string(&mut text).unwrap();
        for labels in [
            "component=\"FrameSender\",operation=\"Connect\",outcome=\"Failed\"",
            "component=\"FrameSender\",operation=\"SendFrame\",outcome=\"SkippedBackoff\"",
            "component=\"FrameEncoder\",operation=\"ValidateInput\",outcome=\"Rejected\"",
            "component=\"FrameSender\",operation=\"WriteFrame\",outcome=\"TimedOut\"",
        ] {
            assert!(text.contains(labels), "missing {labels}: {text}");
        }
        // The reconnect test can emit WriteFrame/Failed concurrently.
        assert!(text.contains("codec=\"None\",content=\"Video\",width=\"1920\",height=\"1080\""));
    }

    #[test]
    fn rejects_invalid_or_truncated_settings_without_starting_encoder() {
        for json in [
            r#"{"type":"hello","version":1}"#,
            r#"{"type":"hello","version":2}"#,
            r#"{"type":"capture_settings","refresh_rate_hz":30,"capture_resolution":[3,360]}"#,
            r#"{"type":"capture_settings","refresh_rate_hz":0,"capture_resolution":[640,360]}"#,
            r#"{"type":"capture_settings","refresh_rate_hz":61,"capture_resolution":[640,360]}"#,
        ] {
            let mut bytes = u32::try_from(json.len()).unwrap().to_be_bytes().to_vec();
            bytes.extend_from_slice(json.as_bytes());
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let mut sender = Sender::new(listener.local_addr().unwrap(), Compression::H264);
            let receiver = thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                read_hello(&mut socket);
                socket.write_all(&bytes).unwrap();
            });
            assert!(sender.connect().is_err());
            assert!(!sender.connected());
            assert!(!sender.ready());
            assert!(sender.encoder.is_none());
            receiver.join().unwrap();
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut sender = Sender::new(listener.local_addr().unwrap(), Compression::None);
        let receiver = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            read_hello(&mut socket);
            socket.write_all(&30u32.to_be_bytes()).unwrap();
            socket.write_all(b"{").unwrap();
        });
        assert!(sender.connect().is_err());
        receiver.join().unwrap();
    }

    #[test]
    fn stalled_handshake_times_out_and_enters_backoff() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut sender = Sender::new(listener.local_addr().unwrap(), Compression::None);
        let receiver = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            read_hello(&mut socket);
            let mut eof = [0; 1];
            assert_eq!(socket.read(&mut eof).unwrap(), 0);
        });
        let start = Instant::now();
        let error = sender.connect().unwrap_err();
        assert_eq!(
            error.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::TimedOut
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(!sender.connected());
        assert!(!sender.ready());
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
