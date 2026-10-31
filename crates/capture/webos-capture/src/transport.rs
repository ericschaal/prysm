use crate::cli::Compression;
use anyhow::{Context, Result};
use nix::{
    errno::Errno,
    poll::{PollFd, PollFlags, poll},
};
use prysm_metrics::{
    BytesKind, Codec, Content, EncoderEvent, FrameSenderEvent, Stage, WireBoundary,
};
use prysm_webos_protocol::{Header, compress};
use std::{
    io::{self, IoSlice, Write},
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
        }
    }

    pub fn ready(&self) -> bool {
        self.socket.is_some() || Instant::now() >= self.retry_after
    }

    /// Reconnect backoff skips work; capture keeps running without a frame queue.
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
        if !self.ready() {
            prysm_metrics::event(FrameSenderEvent::BackoffSkipped);
            return Ok(SendOutcome::Backoff);
        }
        let result: Result<SendOutcome> = (|| {
            if self.socket.is_none() {
                prysm_metrics::event(FrameSenderEvent::ConnectAttempt);
                let connect_time = prysm_metrics::timer(Stage::FrameConnect);
                let connected = TcpStream::connect_timeout(&self.destination, DEADLINE)
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
                self.socket = Some(connected);
                prysm_metrics::event(if self.has_connected {
                    FrameSenderEvent::Reconnected
                } else {
                    FrameSenderEvent::Connected
                });
                self.has_connected = true;
            }
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
mod tests {
    use super::*;
    use prysm_webos_protocol::{HEADER_LEN, PixelFormat, decompress};
    use std::{io::Read, net::TcpListener, thread};

    #[test]
    fn loopback_raw_and_compressed_frames_have_exact_boundaries() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let receiver = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
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
        sender.fps = 60;
        let receiver = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
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
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().unwrap();
                let mut bytes = [0; HEADER_LEN];
                socket.read_exact(&mut bytes).unwrap();
                let header = Header::decode(&bytes).unwrap();
                assert_eq!(header.codec, prysm_webos_protocol::Codec::H264);
                let mut payload = vec![0; header.payload_len];
                socket.read_exact(&mut payload).unwrap();
                let mut decoder = prysm_webos_protocol::h264::Decoder::new().unwrap();
                assert_eq!(decoder.decode(header, &payload).unwrap().len(), 64 * 32 * 3);
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
        assert_eq!(
            sender.send(header, &pixels, Content::Blended).unwrap(),
            SendOutcome::Sent
        );
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
        assert!(sender.send(header, &[0; 3], Content::Video).is_err());
        assert!(!sender.ready());
        assert_eq!(
            sender.send(header, &[0; 3], Content::Video).unwrap(),
            SendOutcome::Backoff
        );
        assert!(sender.socket.is_none());
        let listener = TcpListener::bind(address).unwrap();
        sender.retry_after = Instant::now();
        assert!(sender.ready());
        assert_eq!(
            sender.send(header, &[1, 2, 3], Content::Video).unwrap(),
            SendOutcome::Sent
        );
        let (mut receiver, _) = listener.accept().unwrap();
        let mut bytes = [0; HEADER_LEN + 3];
        receiver.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes[HEADER_LEN..], &[1, 2, 3]);
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
        assert!(sender.send(header, &[0; 3], Content::Video).is_err());
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
