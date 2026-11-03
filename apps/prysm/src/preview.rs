//! Demand-driven diagnostic encoding; never participates in the lighting stop barrier.
use anyhow::{Context, Result, bail};
use axum::{
    Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use prysm_contract::ConfigRevision;
use prysm_core::{ColorStrip, PixelFormat, ProcessedSnapshot};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::Duration,
};
use tokio::sync::{Semaphore, oneshot, watch};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub(crate) struct Publisher {
    latest: watch::Sender<Option<PublishedSnapshot>>,
    sequence: Arc<AtomicU64>,
}
#[derive(Clone)]
struct PublishedSnapshot {
    id: u64,
    revision: ConfigRevision,
    snapshot: ProcessedSnapshot,
}
impl Publisher {
    pub(crate) fn publish(&self, snapshot: ProcessedSnapshot, revision: ConfigRevision) {
        self.latest.send_replace(Some(PublishedSnapshot {
            id: self.sequence.fetch_add(1, Ordering::Relaxed),
            revision,
            snapshot,
        }));
    }
}

type Job = oneshot::Sender<Arc<str>>;
#[derive(Default)]
struct Worker {
    jobs: Option<mpsc::SyncSender<Job>>,
    join: Option<JoinHandle<()>>,
}
struct Shared {
    publisher: Publisher,
    worker: Mutex<Worker>,
    clients: Arc<Semaphore>,
    shutdown: CancellationToken,
}
#[derive(Clone)]
pub(crate) struct Preview(Arc<Shared>);
impl Preview {
    pub(crate) fn new() -> Self {
        let (latest, _) = watch::channel(None);
        Self(Arc::new(Shared {
            publisher: Publisher {
                latest,
                sequence: Arc::new(AtomicU64::new(1)),
            },
            worker: Mutex::new(Worker::default()),
            clients: Arc::new(Semaphore::new(4)),
            shutdown: CancellationToken::new(),
        }))
    }
    pub(crate) fn publisher(&self) -> Publisher {
        self.0.publisher.clone()
    }
    pub(crate) fn router(&self) -> Router {
        Router::new()
            .route("/api/preview", get(upgrade))
            .with_state(self.clone())
    }
    fn request(&self) -> Result<oneshot::Receiver<Arc<str>>> {
        anyhow::ensure!(!self.0.shutdown.is_cancelled(), "preview is shutting down");
        let mut worker = self.0.worker.lock().expect("preview worker lock poisoned");
        if worker.jobs.is_none() {
            let (jobs, receiver) = mpsc::sync_channel::<Job>(1);
            let latest = self.0.publisher.latest.subscribe();
            let join = std::thread::Builder::new()
                .name("prysm-preview".into())
                .spawn(move || encode_jobs(&receiver, &latest, encode))
                .context("cannot start preview encoder")?;
            worker.jobs = Some(jobs);
            worker.join = Some(join);
            tracing::debug!("preview encoder started");
        }
        let (reply, receiver) = oneshot::channel();
        worker
            .jobs
            .as_ref()
            .expect("worker started")
            .try_send(reply)
            .context("preview encoder busy")?;
        Ok(receiver)
    }
    pub(crate) async fn shutdown(&self) -> Result<()> {
        self.0.shutdown.cancel();
        let join = {
            let mut worker = self.0.worker.lock().expect("preview worker lock poisoned");
            worker.jobs.take();
            worker.join.take()
        };
        if let Some(join) = join {
            tokio::task::spawn_blocking(move || join.join())
                .await
                .context("preview join failed")?
                .map_err(|_| anyhow::anyhow!("preview encoder panicked"))?;
        }
        // Permits are acquired before upgrading, so this observes all upgraded sockets
        // even when no encoder worker was ever needed.
        let _closed = self
            .0
            .clients
            .clone()
            .acquire_many_owned(4)
            .await
            .context("preview viewer barrier closed")?;
        Ok(())
    }
}
fn encode_jobs(
    receiver: &mpsc::Receiver<Job>,
    latest: &watch::Receiver<Option<PublishedSnapshot>>,
    mut encode_snapshot: impl FnMut(&PublishedSnapshot) -> Result<String>,
) {
    let mut cached: Option<(u64, Arc<str>)> = None;
    let mut stats = prysm_metrics::TraceStats::new("preview_encode");
    while let Ok(reply) = receiver.recv() {
        if reply.is_closed() {
            continue;
        }
        let current = latest.borrow().clone();
        let response = match current {
            None => Arc::from(r#"{"type":"idle"}"#),
            Some(current) => {
                if let Some((previous, encoded)) = &cached
                    && *previous == current.id
                {
                    stats.cache_hit();
                    let _ = reply.send(encoded.clone());
                    continue;
                }
                let started = stats.start();
                let encoded = match encode_snapshot(&current) {
                    Ok(encoded) => {
                        stats.record(started, encoded.len(), None);
                        Arc::from(encoded)
                    }
                    Err(error) => {
                        if stats.failure() {
                            tracing::warn!(error = %error, "preview encoding failed");
                        }
                        Arc::from(
                            serde_json::json!({"type":"error", "message":format!("{error:#}")})
                                .to_string(),
                        )
                    }
                };
                cached = Some((current.id, Arc::clone(&encoded)));
                encoded
            }
        };
        let _ = reply.send(response);
    }
}
async fn upgrade(State(preview): State<Preview>, ws: WebSocketUpgrade) -> Response {
    if preview.0.shutdown.is_cancelled() {
        return (StatusCode::SERVICE_UNAVAILABLE, "preview is shutting down").into_response();
    }
    let Ok(permit) = preview.0.clients.clone().try_acquire_owned() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "preview viewer limit reached",
        )
            .into_response();
    };
    ws.max_message_size(32)
        .max_frame_size(32)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            connection(preview, socket).await;
        })
}
async fn connection(preview: Preview, mut socket: WebSocket) {
    tracing::debug!(
        viewers = 4 - preview.0.clients.available_permits(),
        "preview viewer connected"
    );
    let mut responses = 0_u64;
    let mut busy = 0_u64;
    let mut next = tokio::time::Instant::now();
    loop {
        let request = tokio::select! {
            () = preview.0.shutdown.cancelled() => break,
            message = socket.recv() => message,
        };
        match request {
            Some(Ok(Message::Text(text))) if text == "next" => {}
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            _ => break,
        }
        tokio::select! {
            () = preview.0.shutdown.cancelled() => break,
            () = tokio::time::sleep_until(next) => {},
        }
        let response = if let Ok(reply) = preview.request() {
            tokio::select! {
                () = preview.0.shutdown.cancelled() => break,
                result = reply => match result { Ok(value) => value, Err(_) => break },
            }
        } else {
            busy += 1;
            Arc::from(r#"{"type":"busy"}"#)
        };
        next = tokio::time::Instant::now() + Duration::from_millis(125);
        let send = socket.send(Message::Text(response.to_string().into()));
        tokio::select! {
            () = preview.0.shutdown.cancelled() => break,
            result = tokio::time::timeout(Duration::from_secs(3), send) => {
                if !matches!(result, Ok(Ok(()))) {
                    tracing::debug!("preview viewer send failed or timed out");
                    break;
                }
                responses += 1;
            },
        }
    }
    tracing::debug!(responses, busy, "preview viewer disconnected");
}
fn strip(strip: &ColorStrip) -> Vec<[u8; 3]> {
    // Diagnostic perimeter resolution stays bounded independently of input/sample settings.
    (0..strip.len().min(256))
        .map(|i| {
            let color = strip
                .color_at(i, strip.len().min(256))
                .expect("bounded strip index")
                .to_srgb();
            [color.r, color.g, color.b]
        })
        .collect()
}
#[allow(
    clippy::cast_possible_truncation,
    reason = "preview sizes are capped at 480 and source coordinates stay within validated u32 dimensions"
)]
fn encode(published: &PublishedSnapshot) -> Result<String> {
    let snapshot = &published.snapshot;
    let frame = &snapshot.frame;
    anyhow::ensure!(
        frame.width() > 0 && frame.height() > 0,
        "preview source is empty"
    );
    let longest = u64::from(frame.width().max(frame.height())).max(480);
    let width = (u64::from(frame.width()) * 480 / longest).max(1) as u32;
    let height = (u64::from(frame.height()) * 480 / longest).max(1) as u32;
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for y in 0..height {
        let source_y = (u64::from(y) * u64::from(frame.height()) / u64::from(height)) as u32;
        for x in 0..width {
            let source_x = (u64::from(x) * u64::from(frame.width()) / u64::from(width)) as u32;
            let offset = (source_y as usize * frame.width() as usize + source_x as usize) * 3;
            let (r, g, b) = match frame.format() {
                PixelFormat::RGB24 => {
                    let pixel = &frame.as_slice()[offset..offset + 3];
                    (pixel[0], pixel[1], pixel[2])
                }
                PixelFormat::BGR24 => {
                    let pixel = &frame.as_slice()[offset..offset + 3];
                    (pixel[2], pixel[1], pixel[0])
                }
                PixelFormat::YUYV { range, matrix } => prysm_core::yuyv::yuyv_pixel_to_rgb(
                    frame.as_slice(),
                    source_x,
                    source_y,
                    frame.width(),
                    range,
                    matrix,
                ),
                PixelFormat::MJPEG => bail!("MJPEG preview is unsupported"),
            };
            rgb.extend_from_slice(&[r, g, b]);
        }
    }
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 75)
        .encode(&rgb, width, height, image::ExtendedColorType::Rgb8)
        .context("cannot encode preview JPEG")?;
    let viewport = snapshot
        .viewport
        .map(|v| serde_json::json!({"x":v.x,"y":v.y,"width":v.width,"height":v.height}));
    let colors = &snapshot.colors;
    Ok(serde_json::json!({
        "type":"frame", "frame_id":published.id, "revision":published.revision,
        "source_width":frame.width(), "source_height":frame.height(), "width":width, "height":height,
        "viewport":viewport,
        "colors":{"top":strip(&colors.top),"right":strip(&colors.right),"bottom":strip(&colors.bottom),"left":strip(&colors.left)},
        "jpeg": STANDARD.encode(jpeg)
    }).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_core::{EdgeColors, Frame, Viewport};

    #[tokio::test]
    async fn cached_response_does_not_pin_the_previous_raw_frame() {
        let preview = Preview::new();
        let mut pool = prysm_core::FramePool::default();
        let mut writer = pool.acquire(4, 2, PixelFormat::RGB24).unwrap();
        writer.as_mut_slice().fill(42);
        let frame = writer.finish();
        let pixels = frame.as_slice().as_ptr();
        preview.publisher().publish(
            ProcessedSnapshot {
                frame,
                viewport: None,
                colors: EdgeColors::default(),
            },
            ConfigRevision::new(1),
        );
        let response = preview.request().unwrap().await.unwrap();
        assert!(response.contains("\"frame_id\":1"));
        preview.publisher().publish(
            ProcessedSnapshot {
                frame: Frame::dummy(4, 2).unwrap(),
                viewport: None,
                colors: EdgeColors::default(),
            },
            ConfigRevision::new(2),
        );
        // The response remains held while the encoder has no further demand.
        let writer = pool.acquire(4, 2, PixelFormat::RGB24).unwrap();
        assert_eq!(writer.finish().as_slice().as_ptr(), pixels);
        preview.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn lazy_worker_encodes_paired_result_and_reuses_same_frame() {
        let preview = Preview::new();
        assert!(preview.0.worker.lock().unwrap().join.is_none());
        preview.publisher().publish(
            ProcessedSnapshot {
                frame: Frame::fill(255, 8, 4, PixelFormat::RGB24).unwrap(),
                viewport: Some(Viewport {
                    x: 2,
                    y: 0,
                    width: 4,
                    height: 4,
                }),
                colors: EdgeColors::default(),
            },
            ConfigRevision::new(7),
        );
        assert!(preview.0.worker.lock().unwrap().join.is_none());
        let first = preview.request().unwrap().await.unwrap();
        let data: serde_json::Value = serde_json::from_str(&first).unwrap();
        assert_eq!(data["revision"], "7");
        assert_eq!(data["viewport"]["x"], 2);
        assert_eq!(data["source_width"], 8);
        let jpeg = STANDARD.decode(data["jpeg"].as_str().unwrap()).unwrap();
        assert_eq!(&jpeg[..2], &[255, 216]);
        let second = preview.request().unwrap().await.unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        preview.shutdown().await.unwrap();
        assert!(preview.request().is_err());
    }
    #[tokio::test]
    async fn queue_and_connection_demand_are_bounded() {
        let preview = Preview::new();
        let (jobs, _receiver) = mpsc::sync_channel(1);
        preview.0.worker.lock().unwrap().jobs = Some(jobs);
        let _queued = preview.request().unwrap();
        assert!(preview.request().is_err());
        let permits: Vec<_> = (0..4)
            .map(|_| preview.0.clients.clone().try_acquire_owned().unwrap())
            .collect();
        assert!(preview.0.clients.clone().try_acquire_owned().is_err());
        let shutdown = preview.shutdown();
        tokio::pin!(shutdown);
        assert!(
            futures::poll!(&mut shutdown).is_pending(),
            "application shutdown must observe upgraded connection permits"
        );
        drop(permits);
        shutdown.await.unwrap();
    }
    #[test]
    fn portrait_and_bgr_preview_preserve_geometry_and_channels() {
        let mut data = Vec::new();
        for _ in 0..(600 * 1200) {
            data.extend_from_slice(&[0, 0, 255]);
        }
        let published = PublishedSnapshot {
            id: 1,
            revision: ConfigRevision::new(2),
            snapshot: ProcessedSnapshot {
                frame: Frame::new(data, 600, 1200, PixelFormat::BGR24).unwrap(),
                viewport: Some(Viewport {
                    x: 10,
                    y: 20,
                    width: 500,
                    height: 1000,
                }),
                colors: EdgeColors::default(),
            },
        };
        let value: serde_json::Value = serde_json::from_str(&encode(&published).unwrap()).unwrap();
        assert_eq!(value["width"], 240);
        assert_eq!(value["height"], 480);
        assert_eq!(value["viewport"]["width"], 500);
        let jpeg = STANDARD.decode(value["jpeg"].as_str().unwrap()).unwrap();
        let decoded = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        assert_eq!(decoded.dimensions(), (240, 480));
        let pixel = decoded.get_pixel(100, 100).0;
        assert!(
            pixel[0] > 240 && pixel[1] < 10 && pixel[2] < 10,
            "{pixel:?}"
        );
    }
    #[tokio::test]
    async fn stalled_encoder_does_not_hold_snapshot_or_accumulate_old_frames() {
        let preview = Preview::new();
        let publisher = preview.publisher();
        let snapshot = ProcessedSnapshot {
            frame: Frame::dummy(4, 2).unwrap(),
            viewport: None,
            colors: EdgeColors::default(),
        };
        publisher.publish(snapshot.clone(), ConfigRevision::new(1));
        let (jobs, receiver) = mpsc::sync_channel(1);
        let latest = publisher.latest.subscribe();
        let (started, ready) = oneshot::channel();
        let (release, gate) = mpsc::channel();
        let join = std::thread::spawn(move || {
            let mut started = Some(started);
            encode_jobs(&receiver, &latest, move |frame| {
                if let Some(started) = started.take() {
                    let _ = started.send(());
                    gate.recv().unwrap();
                }
                Ok(frame.id.to_string())
            });
        });
        let (reply, active) = oneshot::channel();
        jobs.try_send(reply).unwrap();
        ready.await.unwrap();
        let (reply, queued) = oneshot::channel();
        jobs.try_send(reply).unwrap();
        let (reply, _rejected) = oneshot::channel();
        assert!(jobs.try_send(reply).is_err());
        for _ in 0..1000 {
            publisher.publish(snapshot.clone(), ConfigRevision::new(2));
        }
        // This current-thread runtime remains free while the synchronous encoder is gated.
        tokio::task::yield_now().await;
        release.send(()).unwrap();
        assert_eq!(&*active.await.unwrap(), "1");
        assert_eq!(
            &*queued.await.unwrap(),
            "1001",
            "queued demand must select the newest snapshot when work begins"
        );
        drop(jobs);
        tokio::task::spawn_blocking(move || join.join())
            .await
            .unwrap()
            .unwrap();
        preview.shutdown().await.unwrap();
    }
}
