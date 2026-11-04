//! One native WS/WSS connection. Each binary message carries one control or one frame.
use crate::{config::CaptureOptions, encoder::EncodedFrame};
use anyhow::{Context, Result, bail, ensure};
use nix::poll::{PollFd, PollFlags, poll};
use prysm_metrics::{BytesKind, Codec, Content, FrameSenderEvent, Stage, WireBoundary};
use prysm_webos_protocol::{ControlMessage, HELLO, InstanceUrl, MAX_CONTROL_LEN};
use std::{
    io,
    net::{SocketAddr, TcpStream},
    os::fd::AsFd,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use tungstenite::{Message, WebSocket, protocol::WebSocketConfig, stream::MaybeTlsStream};

const HANDSHAKE: Duration = Duration::from_secs(4);
const FRAME_DEADLINE: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub struct ReceiverBusy;
impl std::fmt::Display for ReceiverBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Receiver in use")
    }
}
impl std::error::Error for ReceiverBusy {}

pub struct Sender {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    tcp: TcpStream,
}

impl Sender {
    pub fn connect(instance: &InstanceUrl) -> Result<(Self, ControlMessage)> {
        prysm_metrics::event(FrameSenderEvent::ConnectAttempt);
        let url = instance.capture_url();
        let addresses = url.socket_addrs(|| url.port_or_known_default())?;
        let tcp = connect_tcp(&addresses)
            .inspect_err(|e| {
                prysm_metrics::event(if e.kind() == io::ErrorKind::TimedOut {
                    FrameSenderEvent::ConnectTimedOut
                } else {
                    FrameSenderEvent::ConnectFailed
                });
            })
            .context("could not connect to Prysm")?;
        tcp.set_nodelay(true)?;
        tcp.set_read_timeout(Some(HANDSHAKE))?;
        tcp.set_write_timeout(Some(HANDSHAKE))?;
        let _ = rustls::crypto::ring::default_provider().install_default();
        let controls = WebSocketConfig::default()
            .max_message_size(Some(MAX_CONTROL_LEN))
            .max_frame_size(Some(MAX_CONTROL_LEN));
        let (mut socket, _) = tungstenite::client_tls_with_config(
            url.as_str(),
            tcp.try_clone()?,
            Some(controls),
            None,
        )
        .context("Prysm WebSocket upgrade or TLS verification failed")?;
        socket.send(Message::binary(HELLO.encode()?))?;
        let Message::Binary(bytes) = socket.read()? else {
            bail!("expected a Prysm handshake")
        };
        let reply = ControlMessage::decode(&bytes)?;
        ensure!(
            !matches!(reply, ControlMessage::Hello { .. }),
            "invalid Prysm handshake"
        );
        tcp.set_nonblocking(true)?;
        prysm_metrics::event(FrameSenderEvent::Connected);
        Ok((Self { socket, tcp }, reply))
    }

    fn control(&mut self) -> Result<Option<ControlMessage>> {
        loop {
            match self.socket.read() {
                Ok(Message::Binary(bytes)) => return Ok(Some(ControlMessage::decode(&bytes)?)),
                Ok(Message::Ping(_) | Message::Pong(_)) => {}
                Err(ref error) if blocked(error) => return Ok(None),
                Err(error) => return Err(error.into()),
                _ => bail!("Prysm closed the capture connection"),
            }
        }
    }
    pub fn check_connection(&mut self) -> Result<()> {
        ensure!(self.control()?.is_none(), "unexpected capture control");
        Ok(())
    }
    pub fn wait_for_capture(
        &mut self,
        mut message: ControlMessage,
        stop: &AtomicBool,
    ) -> Result<Option<CaptureOptions>> {
        while !stop.load(Ordering::Relaxed) {
            match message {
                ControlMessage::CaptureSettings {
                    refresh_rate_hz,
                    capture_resolution: [width, height],
                } => {
                    return Ok(Some(CaptureOptions {
                        fps: refresh_rate_hz.try_into()?,
                        width,
                        height,
                    }));
                }
                ControlMessage::Busy => return Err(ReceiverBusy.into()),
                ControlMessage::Idle => (),
                ControlMessage::Hello { .. } => bail!("expected capture settings"),
            }
            if let Some(next) = self.control()? {
                message = next;
            } else {
                poll(
                    &mut [PollFd::new(self.tcp.as_fd(), PollFlags::POLLIN)],
                    100_u16,
                )?;
            }
        }
        Ok(None)
    }
    pub fn send(&mut self, frame: EncodedFrame<'_>, content: Content) -> Result<()> {
        let _time = prysm_metrics::timer(Stage::FrameWrite);
        let header = frame.header;
        let mut bytes = Vec::with_capacity(prysm_webos_protocol::HEADER_LEN + frame.pixels.len());
        bytes.extend_from_slice(&header.encode()?);
        bytes.extend_from_slice(frame.pixels);
        let len = bytes.len();
        let deadline = Instant::now() + FRAME_DEADLINE;
        let mut result = self.socket.send(Message::binary(bytes));
        while let Err(ref error) = result {
            if !blocked(error) {
                prysm_metrics::event(FrameSenderEvent::FrameWriteFailed);
                break;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                prysm_metrics::event(FrameSenderEvent::FrameWriteTimedOut);
                bail!("timed out writing capture frame");
            }
            let _wait = prysm_metrics::timer(Stage::FrameWriteWait);
            poll(
                &mut [PollFd::new(self.tcp.as_fd(), PollFlags::POLLOUT)],
                u16::try_from(remaining.as_millis() + 1)?,
            )?;
            // Tungstenite retains the unsent suffix. Resending the message would duplicate it.
            result = self.socket.flush();
        }
        result?;
        prysm_metrics::bytes(BytesKind::SenderSocketAccepted, len);
        prysm_metrics::wire_frame(
            WireBoundary::SocketAccepted,
            header.format.into(),
            Codec::from(&header),
            content,
            header.width,
            header.height,
        );
        Ok(())
    }
    pub fn disconnect(&self) {
        let _ = self.tcp.shutdown(std::net::Shutdown::Both);
    }
}
fn connect_tcp(addresses: &[SocketAddr]) -> io::Result<TcpStream> {
    let _timer = prysm_metrics::timer(Stage::FrameConnect);
    let mut error = io::Error::other("no address for Prysm");
    let deadline = Instant::now() + HANDSHAKE;
    for (i, address) in addresses.iter().enumerate() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        // Give each resolved address a chance when a host's IPv6 route is unavailable.
        let attempts = u32::try_from(addresses.len() - i).unwrap_or(u32::MAX);
        match TcpStream::connect_timeout(address, remaining / attempts) {
            Ok(socket) => return Ok(socket),
            Err(cause) => error = cause,
        }
    }
    Err(error)
}
fn blocked(error: &tungstenite::Error) -> bool {
    matches!(error,tungstenite::Error::Io(e) if e.kind()==io::ErrorKind::WouldBlock)
}

#[cfg(test)]
pub(crate) mod tests;
