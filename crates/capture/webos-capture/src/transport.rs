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
}

pub struct Sender {
    destination: SocketAddr,
    socket: Option<TcpStream>,
    retry_after: Instant,
    compression: Compression,
    scratch: Vec<u8>,
    has_connected: bool,
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
            header.compressed = false;
            let encode_time = prysm_metrics::timer(Stage::FrameEncode);
            let payload = if self.compression == Compression::Lz4 {
                if let Some(length) = compress(pixels, &mut self.scratch)
                    .inspect_err(|_| prysm_metrics::event(EncoderEvent::CompressionFailed))?
                {
                    header.compressed = true;
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
                assert_eq!(header.compressed, compressed);
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
            compressed: false,
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
    fn refuses_connection_then_backs_off_without_queuing() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let mut sender = Sender::new(address, Compression::None);
        let header = Header {
            width: 1,
            height: 1,
            format: PixelFormat::Rgb,
            compressed: false,
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
            compressed: false,
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
            compressed: false,
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
        assert!(!text.contains("operation=\"WriteFrame\",outcome=\"Failed\""));
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
