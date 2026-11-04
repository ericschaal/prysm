//! One native WS/WSS connection. Each binary message carries one control or one frame.
use crate::{config::CaptureOptions, encoder::EncodedFrame};
use anyhow::{Context, Result, bail, ensure};
use futures::{FutureExt, SinkExt, StreamExt};
use prysm_metrics::{BytesKind, Codec, Content, FrameSenderEvent, Stage, WireBoundary};
use prysm_webos_protocol::{CONTROL_TIMEOUT, ControlMessage, HELLO, InstanceUrl, MAX_CONTROL_LEN};
use std::{io, os::fd::AsRawFd, time::Duration};
use tokio::{
    net::TcpStream,
    runtime::Runtime,
    time::{Instant, timeout, timeout_at},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, client_async_tls_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};

const FRAME_DEADLINE: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub struct ReceiverBusy;
impl std::fmt::Display for ReceiverBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Receiver in use")
    }
}
impl std::error::Error for ReceiverBusy {}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub struct Sender {
    socket: Socket,
    runtime: Runtime,
}

impl Sender {
    pub fn connect(instance: &InstanceUrl) -> Result<(Self, ControlMessage)> {
        // Native capture stays on the caller's thread. Socket readiness must advance between frames.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()?;
        prysm_metrics::event(FrameSenderEvent::ConnectAttempt);
        let result = runtime.block_on(async {
            let deadline = Instant::now() + CONTROL_TIMEOUT;
            let url = instance.capture_url();
            let port = url
                .port_or_known_default()
                .context("missing capture port")?;
            let authority = format!("{}:{port}", url.host_str().context("missing capture host")?);
            let tcp = timeout_at(deadline, connect_tcp(authority, deadline))
                .await
                .map_err(io::Error::from)
                .flatten()
                .context("could not connect to Prysm")?;
            tcp.set_nodelay(true)?;
            let _ = rustls::crypto::ring::default_provider().install_default();
            let controls = WebSocketConfig::default()
                .max_message_size(Some(MAX_CONTROL_LEN))
                .max_frame_size(Some(MAX_CONTROL_LEN));
            timeout_at(deadline, async {
                let (mut socket, _) =
                    client_async_tls_with_config(url.as_str(), tcp, Some(controls), None)
                        .await
                        .context("Prysm WebSocket upgrade or TLS verification failed")?;
                socket.send(Message::binary(HELLO.encode()?)).await?;
                let reply = read_control(&mut socket).await?;
                ensure!(
                    !matches!(reply, ControlMessage::Hello { .. }),
                    "invalid Prysm handshake"
                );
                anyhow::Ok((socket, reply))
            })
            .await
            .map_err(io::Error::from)?
        });
        match result {
            Ok((socket, reply)) => {
                prysm_metrics::event(FrameSenderEvent::Connected);
                Ok((Self { socket, runtime }, reply))
            }
            Err(error) => {
                let timed_out = error
                    .downcast_ref::<io::Error>()
                    .is_some_and(|e| e.kind() == io::ErrorKind::TimedOut);
                prysm_metrics::event(if timed_out {
                    FrameSenderEvent::ConnectTimedOut
                } else {
                    FrameSenderEvent::ConnectFailed
                });
                // A timed-out OS hostname lookup can still be running on Tokio's blocking pool.
                runtime.shutdown_background();
                Err(error)
            }
        }
    }

    pub fn check_connection(&mut self) -> Result<()> {
        self.runtime.block_on(async {
            ensure!(
                read_control(&mut self.socket)
                    .now_or_never()
                    .transpose()?
                    .is_none(),
                "unexpected capture control"
            );
            Ok(())
        })
    }
    pub fn wait_for_capture(
        &mut self,
        mut message: ControlMessage,
        mut ready: impl FnMut() -> Result<bool>,
    ) -> Result<Option<CaptureOptions>> {
        self.runtime.block_on(async {
            while ready()? {
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
                let reply = timeout(CONTROL_TIMEOUT, read_control(&mut self.socket));
                tokio::pin!(reply);
                loop {
                    tokio::select! {
                        result = &mut reply => {
                            message = result.map_err(io::Error::from)??;
                            break;
                        }
                        () = tokio::time::sleep(Duration::from_millis(100)) => {
                            if !ready()? {
                                return Ok(None);
                            }
                        }
                    }
                }
            }
            Ok(None)
        })
    }
    pub fn send(&mut self, frame: EncodedFrame<'_>, content: Content) -> Result<()> {
        let _time = prysm_metrics::timer(Stage::FrameWrite);
        let header = frame.header;
        let mut bytes = Vec::with_capacity(prysm_webos_protocol::HEADER_LEN + frame.pixels.len());
        bytes.extend_from_slice(&header.encode()?);
        bytes.extend_from_slice(frame.pixels);
        let len = bytes.len();
        self.runtime.block_on(async {
            let deadline = Instant::now() + FRAME_DEADLINE;
            let sending = self.socket.send(Message::binary(bytes));
            tokio::pin!(sending);
            let result = if let Some(result) = sending.as_mut().now_or_never() {
                result
            } else {
                let _wait = prysm_metrics::timer(Stage::FrameWriteWait);
                timeout_at(deadline, sending).await.map_err(|_| {
                    prysm_metrics::event(FrameSenderEvent::FrameWriteTimedOut);
                    io::Error::new(io::ErrorKind::TimedOut, "timed out writing capture frame")
                })?
            };
            result.inspect_err(|_| prysm_metrics::event(FrameSenderEvent::FrameWriteFailed))
        })?;
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
        let _ = nix::sys::socket::shutdown(
            self.socket.get_ref().get_ref().as_raw_fd(),
            nix::sys::socket::Shutdown::Both,
        );
    }
}

async fn read_control(socket: &mut Socket) -> Result<ControlMessage> {
    loop {
        match socket.next().await.transpose()? {
            Some(Message::Binary(bytes)) => return ControlMessage::decode(&bytes),
            Some(Message::Ping(_) | Message::Pong(_)) => (),
            _ => bail!("Prysm closed the capture connection"),
        }
    }
}

async fn connect_tcp(authority: String, deadline: Instant) -> io::Result<TcpStream> {
    let _timer = prysm_metrics::timer(Stage::FrameConnect);
    let addresses: Vec<_> = tokio::net::lookup_host(authority).await?.collect();
    let mut error = io::Error::other("no address for Prysm");
    for (i, address) in addresses.iter().enumerate() {
        // Give each resolved address a chance when a host's IPv6 route is unavailable.
        let attempts = u32::try_from(addresses.len() - i).unwrap_or(u32::MAX);
        let remaining = deadline.saturating_duration_since(Instant::now());
        match timeout(remaining / attempts, TcpStream::connect(address)).await {
            Ok(Ok(socket)) => return Ok(socket),
            Ok(Err(cause)) => error = cause,
            Err(elapsed) => error = elapsed.into(),
        }
    }
    Err(error)
}

#[cfg(test)]
pub(crate) mod tests;
