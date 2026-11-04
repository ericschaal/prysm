//! One application-owned capture endpoint, independent of pipeline starts and stops.
use crate::{Frame, Result, capture_channel};
use anyhow::{Context, ensure};
use axum::{
    Router,
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
};
use futures::Stream;
use prysm_webos_protocol::{CAPTURE_PATH, ControlMessage, HEADER_LEN, HELLO};
use std::{
    num::NonZeroU32,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{Semaphore, watch};
use tokio_util::sync::{CancellationToken, DropGuard};

#[derive(Clone)]
pub struct WebosReceiver {
    worker: Arc<Worker>,
    capture: watch::Sender<Option<Arc<Capture>>>,
    turn: Arc<tokio::sync::Mutex<()>>,
    clients: Arc<Semaphore>,
}

struct Worker {
    runtime: tokio::runtime::Handle,
    stop: CancellationToken,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
pub(super) struct Capture {
    last_frame: Mutex<Option<Instant>>,
    settings: ControlMessage,
    receiving: watch::Sender<bool>,
    frames: watch::Sender<Option<Frame>>,
    _metrics: prysm_metrics::ReceiverRequest,
    _completed: DropGuard,
}

impl Capture {
    pub(super) fn publish(&self, frame: Frame) -> bool {
        if prysm_metrics::enabled() {
            let now = Instant::now();
            let mut previous = self
                .last_frame
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(last) = previous.replace(now) {
                prysm_metrics::observe(
                    prysm_metrics::Stage::ReceiverFrameGap,
                    now.duration_since(last),
                );
            }
        }
        prysm_metrics::frame(prysm_metrics::FrameStage::Decoded, frame.format().into());
        if self.frames.send(Some(frame)).is_err() {
            return false;
        }
        self.receiving
            .send_if_modified(|available| !std::mem::replace(available, true));
        true
    }
}

impl WebosReceiver {
    pub fn new() -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("create capture runtime")?;
        let handle = runtime.handle().clone();
        let stop = CancellationToken::new();
        let stopped = stop.clone();
        let (capture, _) = watch::channel(None);
        let slot = capture.clone();
        let thread = std::thread::Builder::new()
            .name("prysm-webos-receiver".into())
            .spawn(move || {
                runtime.block_on(stopped.cancelled());
                slot.send_replace(None);
            })
            .context("start capture receiver")?;
        Ok(Self {
            worker: Arc::new(Worker {
                runtime: handle,
                stop,
                thread: Mutex::new(Some(thread)),
            }),
            capture,
            turn: Arc::default(),
            clients: Arc::new(Semaphore::new(16)),
        })
    }

    pub fn router(&self) -> Router {
        let receiver = self.clone();
        Router::new().route(CAPTURE_PATH, get(move |ws: WebSocketUpgrade| async move {
            let Ok(client) = receiver.clients.clone().try_acquire_owned() else {
                return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
            };
            let capture = receiver.capture.subscribe();
            let turn = receiver.turn.clone();
            let handle = receiver.worker.runtime.clone();
            ws.max_message_size(8_388_608 + HEADER_LEN).max_frame_size(8_388_608 + HEADER_LEN)
                .on_upgrade(move |socket| async move {
                    let _ = handle.spawn(async move {
                        let _client = client;
                        if let Err(error) = connection(socket, turn, capture).await {
                            tracing::debug!(error = %format!("{error:#}"), "capture connection ended");
                        }
                    }).await;
                })
        }))
    }

    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.worker.stop.cancel();
        let thread = self
            .worker
            .thread
            .lock()
            .map_err(|_| anyhow::anyhow!("receiver lock poisoned"))?
            .take();
        if let Some(thread) = thread {
            tokio::task::spawn_blocking(move || thread.join())
                .await?
                .map_err(|_| anyhow::anyhow!("receiver panicked"))?;
        }
        Ok(())
    }

    pub fn capture(
        &self,
        width: u32,
        height: u32,
        rate: NonZeroU32,
        shutdown: CancellationToken,
    ) -> Result<(
        impl Stream<Item = Result<Frame>> + Send + 'static,
        CancellationToken,
        watch::Receiver<bool>,
    )> {
        let settings = ControlMessage::CaptureSettings {
            refresh_rate_hz: rate.get(),
            capture_resolution: [width, height],
        }
        .validate()?;
        let (frames, finished, stream) = capture_channel(shutdown.clone());
        let (receiving, availability) = watch::channel(false);
        let completed = CancellationToken::new();
        let slot = self.capture.clone();
        let published = slot.send_if_modified(|slot| {
            if slot.is_some() || self.worker.stop.is_cancelled() {
                return false;
            }
            *slot = Some(Arc::new(Capture {
                last_frame: Mutex::default(),
                settings,
                receiving,
                frames: frames.clone(),
                _completed: completed.clone().drop_guard(),
                _metrics: prysm_metrics::receiver_request(rate),
            }));
            true
        });
        if !published {
            return Err(anyhow::anyhow!("receiver stopped or capture already active").into());
        }
        self.worker.runtime.spawn(async move {
            tokio::select! { () = shutdown.cancelled() => {}, () = frames.closed() => {} }
            slot.send_replace(None);
            let _ = finished.send(Ok(()));
        });
        Ok((stream, completed, availability))
    }
}

async fn send(socket: &mut WebSocket, message: ControlMessage) -> anyhow::Result<()> {
    tokio::time::timeout(
        Duration::from_secs(2),
        socket.send(Message::Binary(message.encode()?.into())),
    )
    .await??;
    Ok(())
}

pub(super) async fn binary(socket: &mut WebSocket) -> anyhow::Result<axum::body::Bytes> {
    loop {
        match socket.recv().await {
            Some(Ok(Message::Binary(bytes))) => return Ok(bytes),
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            Some(Ok(Message::Close(_))) | None => {
                prysm_metrics::event(prysm_metrics::FrameReceiverEvent::MessageClosed);
                anyhow::bail!("TV disconnected")
            }
            Some(Err(error)) => {
                prysm_metrics::event(prysm_metrics::FrameReceiverEvent::MessageReadFailed);
                return Err(error.into());
            }
            _ => anyhow::bail!("expected binary capture message"),
        }
    }
}

async fn connection(
    mut socket: WebSocket,
    turn: Arc<tokio::sync::Mutex<()>>,
    mut capture: watch::Receiver<Option<Arc<Capture>>>,
) -> anyhow::Result<()> {
    let hello = tokio::time::timeout(Duration::from_secs(2), binary(&mut socket)).await??;
    ensure!(
        ControlMessage::decode(&hello)? == HELLO,
        "expected Prysm hello"
    );
    prysm_metrics::event(prysm_metrics::FrameReceiverEvent::ConnectionAccepted);
    // FIFO among ready TVs. An idle connection never starts capture by itself.
    let place = turn.try_lock().ok();
    let active = capture.borrow().is_some();
    if active && place.is_none() {
        return send(&mut socket, ControlMessage::Busy).await;
    }
    if !active {
        send(&mut socket, ControlMessage::Idle).await?;
    }
    let _turn = if let Some(guard) = place {
        guard
    } else {
        tokio::select! {
            biased;
            guard = turn.lock() => guard,
            _ = requested(&mut capture) => return send(&mut socket,ControlMessage::Busy).await,
            _ = binary(&mut socket) => return Ok(()),
        }
    };
    let request = tokio::select! {
        result = requested(&mut capture) => result?,
        _ = binary(&mut socket) => return Ok(()),
    };
    send(&mut socket, request.settings).await?;
    tokio::select! {
        _ = capture.changed() => {},
        () = crate::webos::receive_frames(&mut socket, &request) => {}
    }
    request.receiving.send_replace(false);
    Ok(())
}

async fn requested(
    capture: &mut watch::Receiver<Option<Arc<Capture>>>,
) -> anyhow::Result<Arc<Capture>> {
    Ok(capture
        .wait_for(Option::is_some)
        .await?
        .as_ref()
        .expect("active capture")
        .clone())
}
