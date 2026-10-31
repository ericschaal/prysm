use crate::cli::Compression;
use anyhow::{Context, Result};
use nix::{
    errno::Errno,
    poll::{PollFd, PollFlags, poll},
};
use prysm_webos_protocol::{Header, compress};
use std::{
    io::{self, IoSlice, Write},
    net::{SocketAddr, TcpStream},
    os::fd::AsFd,
    time::{Duration, Instant},
};

const DEADLINE: Duration = Duration::from_millis(100);

pub struct Sender {
    destination: SocketAddr,
    socket: Option<TcpStream>,
    retry_after: Instant,
    compression: Compression,
    scratch: Vec<u8>,
}

impl Sender {
    pub fn new(destination: SocketAddr, compression: Compression) -> Self {
        Self {
            destination,
            socket: None,
            retry_after: Instant::now(),
            compression,
            scratch: Vec::new(),
        }
    }

    /// False means reconnect backoff; capture keeps running without a frame queue.
    pub fn send(&mut self, mut header: Header, pixels: &[u8]) -> Result<bool> {
        anyhow::ensure!(
            pixels.len() == header.raw_len()?,
            "incorrect sender frame length"
        );
        if self.socket.is_none() && Instant::now() < self.retry_after {
            return Ok(false);
        }
        let result: Result<bool> = (|| {
            if self.socket.is_none() {
                let socket = TcpStream::connect_timeout(&self.destination, DEADLINE)?;
                socket.set_nodelay(true)?;
                socket.set_nonblocking(true)?;
                self.socket = Some(socket);
            }
            let payload = if self.compression == Compression::Lz4 {
                if let Some(length) = compress(pixels, &mut self.scratch)? {
                    header.compressed = true;
                    &self.scratch[..length]
                } else {
                    pixels
                }
            } else {
                pixels
            };
            header.payload_len = payload.len();
            let bytes = header.encode()?;
            write_frame(
                self.socket.as_mut().expect("connected socket"),
                &bytes,
                payload,
                DEADLINE,
            )?;
            Ok(true)
        })();
        if result.is_err() {
            // A partially written frame cannot be resumed on a later connection.
            self.socket = None;
            self.retry_after = Instant::now() + Duration::from_secs(1);
        }
        result.with_context(|| format!("sending capture to {}", self.destination))
    }
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
            Ok(written) => IoSlice::advance_slices(&mut remaining, written),
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
        assert!(sender.send(header, &[42; 600]).unwrap());
        sender.compression = Compression::Lz4;
        assert!(sender.send(header, &[42; 600]).unwrap());
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
        assert!(sender.send(header, &[0; 3]).is_err());
        assert!(!sender.send(header, &[0; 3]).unwrap());
        assert!(sender.socket.is_none());
        let listener = TcpListener::bind(address).unwrap();
        sender.retry_after = Instant::now();
        assert!(sender.send(header, &[1, 2, 3]).unwrap());
        let (mut receiver, _) = listener.accept().unwrap();
        let mut bytes = [0; HEADER_LEN + 3];
        receiver.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes[HEADER_LEN..], &[1, 2, 3]);
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
