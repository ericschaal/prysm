//! One owned processing thread and one independently tracked lighting consumer.
use crate::{
    config::PipelineConfig,
    input::{InputSource, InputStream},
};
use anyhow::{Context, Result, anyhow};
use futures::StreamExt;
use prysm_contract::{ConfigRevision, Output, PipelinePhase, PipelineStatus, ProcessingSettings};
use prysm_core::{EdgeColors, ProcessedSnapshot};
use prysm_processor::PrysmProcessor;
use prysm_renderer_wled::WledRenderer;
use std::thread::JoinHandle;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineExit {
    Completed,
    Stopped,
}
pub(crate) struct Configure {
    pub settings: ProcessingSettings,
    pub revision: ConfigRevision,
    pub reply: oneshot::Sender<Result<()>>,
}
struct PreparedOutput {
    backend: WledRenderer,
    leds: [usize; 4],
}
impl PreparedOutput {
    fn new(output: &Output) -> Result<Self> {
        let Output::Wled { leds, .. } = output;
        Ok(Self {
            backend: WledRenderer::new(output.ddp_address()?)
                .context("cannot create WLED renderer")?,
            leds: leds.counts(),
        })
    }
    fn render(&mut self, colors: &EdgeColors) -> Result<()> {
        self.backend
            .render_edges(colors, self.leds)
            .context("cannot send WLED frame")
    }
}
pub(crate) struct Session {
    pub shutdown: CancellationToken,
    pub configure: mpsc::Sender<Configure>,
    completed: oneshot::Receiver<Result<PipelineExit>>,
    worker: JoinHandle<()>,
    output: tokio::task::JoinHandle<Result<()>>,
}
impl Session {
    pub async fn wait(self) -> Result<PipelineExit> {
        let result = self
            .completed
            .await
            .context("processing worker ended unexpectedly");
        let joined = tokio::task::spawn_blocking(move || self.worker.join()).await;
        let output = self.output.await;
        joined
            .context("cannot join processing worker")?
            .map_err(|_| anyhow!("processing worker panicked"))?;
        output.context("lighting task panicked")??;
        result?
    }
}
pub(crate) fn launch(
    config: PipelineConfig,
    status: watch::Sender<PipelineStatus>,
    preview: Option<crate::preview::Publisher>,
    input: InputSource,
) -> Result<(Session, oneshot::Receiver<Result<()>>)> {
    let shutdown = CancellationToken::new();
    let (configure, commands) = mpsc::channel(8);
    let (ready, readiness) = oneshot::channel();
    let (finished, completed) = oneshot::channel();
    let (prepared, backend) = oneshot::channel();
    let (publication, frames) = watch::channel(None::<EdgeColors>);
    let worker_shutdown = shutdown.clone();
    let worker = std::thread::Builder::new()
        .name("prysm-processing".into())
        .spawn(move || {
            let mut ready = Some(ready);
            let result = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime.block_on(async {
                    if worker_shutdown.is_cancelled() {
                        return Ok(PipelineExit::Stopped);
                    }
                    let output = PreparedOutput::new(&config.output)?;
                    if worker_shutdown.is_cancelled() {
                        return Ok(PipelineExit::Stopped);
                    }
                    let frames = input.into_stream(worker_shutdown.clone())?;
                    // A failed application consumer must still release a successfully opened input.
                    if prepared.send(output).is_err() {
                        worker_shutdown.cancel();
                        let completed = frames.completed.clone();
                        let cleanup = frames.cleanup;
                        drop(frames.frames);
                        if let Some(cleanup) = cleanup {
                            cleanup.await.context("file cleanup task panicked")??;
                        }
                        if let Some(completed) = completed {
                            completed.cancelled().await;
                        }
                        return Err(anyhow!("lighting consumer unavailable during startup"));
                    }
                    if let Some(ready) = ready.take() {
                        let _ = ready.send(Ok(()));
                    }
                    process(
                        config,
                        frames,
                        commands,
                        publication,
                        worker_shutdown.clone(),
                        status,
                        preview,
                    )
                    .await
                }),
                Err(error) => Err(error).context("cannot create processing runtime"),
            };
            worker_shutdown.cancel();
            if let Some(ready) = ready.take() {
                let message = result.as_ref().err().map_or_else(
                    || "processing ended before startup".into(),
                    |error| format!("{error:#}"),
                );
                let _ = ready.send(Err(anyhow!(message)));
            }
            let _ = finished.send(result);
        })
        .context("cannot start processing thread")?;
    let output_shutdown = shutdown.clone();
    let output = tokio::spawn(async move {
        let _cancel_on_exit = output_shutdown.clone().drop_guard();
        let Ok(mut output) = backend.await else {
            return Ok(());
        };
        let result = consume(&mut output, frames).await;
        if result.is_err() {
            output_shutdown.cancel();
        }
        result
    });
    Ok((
        Session {
            shutdown,
            configure,
            completed,
            worker,
            output,
        },
        readiness,
    ))
}
async fn consume(
    output: &mut PreparedOutput,
    mut frames: watch::Receiver<Option<EdgeColors>>,
) -> Result<()> {
    // changed() delivers an unseen final value even after the sole publisher closes.
    while frames.changed().await.is_ok() {
        let snapshot = frames.borrow_and_update().clone();
        if let Some(snapshot) = snapshot {
            output.render(&snapshot)?;
        }
        tokio::task::yield_now().await;
    }
    Ok(())
}
fn availability(status: &watch::Sender<PipelineStatus>, available: bool) {
    status.send_modify(|status| {
        if !matches!(
            status.phase,
            PipelinePhase::Starting | PipelinePhase::Running
        ) {
            return;
        }
        if available {
            if status.frames_processed > 0 {
                status.phase = PipelinePhase::Running;
            }
            status.error = None;
        } else {
            status.phase = PipelinePhase::Starting;
            status.error = Some("input unavailable; waiting for frames".into());
        }
    });
}
async fn process(
    config: PipelineConfig,
    input: InputStream,
    mut commands: mpsc::Receiver<Configure>,
    publication: watch::Sender<Option<EdgeColors>>,
    shutdown: CancellationToken,
    status: watch::Sender<PipelineStatus>,
    preview: Option<crate::preview::Publisher>,
) -> Result<PipelineExit> {
    let InputStream {
        mut frames,
        completed,
        cleanup,
        availability: mut available,
    } = input;
    let mut processor = PrysmProcessor::new(&config.processing.to_core());
    let mut revision = config.revision;
    status.send_modify(|status| status.applied_revision = Some(revision));
    availability(
        &status,
        available.as_ref().is_none_or(|receiver| *receiver.borrow()),
    );
    let result = 'frames: loop {
        // Keep this future alive through configuration/availability events: rawvideo reads
        // may have consumed a partial frame already.
        let next = frames.next();
        tokio::pin!(next);
        let frame = loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => break 'frames Ok(PipelineExit::Stopped),
                command = commands.recv(), if !commands.is_closed() => {
                    if let Some(command) = command {
                        processor.configure(&command.settings.to_core());
                        revision = command.revision;
                        status.send_modify(|status| status.applied_revision = Some(revision));
                        let _ = command.reply.send(Ok(()));
                    }
                },
                result = async {
                    match &mut available {
                        Some(receiver) => receiver.changed().await,
                        None => std::future::pending().await,
                    }
                } => {
                    if result.is_err() { available = None; }
                    availability(&status, available.as_ref().is_none_or(|receiver| *receiver.borrow()));
                },
                frame = &mut next => break frame,
            }
        };
        let frame = match frame {
            Some(Ok(frame)) => frame,
            Some(Err(error)) => break Err(error),
            None => break Ok(PipelineExit::Completed),
        };
        prysm_metrics::frame(
            prysm_metrics::FrameStage::InputDelivered,
            frame.format().into(),
        );
        let timer = prysm_metrics::timer(prysm_metrics::Stage::FrameProcess);
        let colors = processor.process_frame(frame.clone());
        drop(timer);
        prysm_metrics::frame(prysm_metrics::FrameStage::Processed, frame.format().into());
        if let Some(preview) = &preview {
            preview.publish(
                ProcessedSnapshot {
                    frame,
                    viewport: processor.viewport(),
                    colors: colors.clone(),
                },
                revision,
            );
        }
        publication.send_replace(Some(colors));
        status.send_modify(|status| {
            status.frames_processed = status.frames_processed.saturating_add(1);
        });
        availability(
            &status,
            available.as_ref().is_none_or(|receiver| *receiver.borrow()),
        );
        tokio::task::yield_now().await;
    };
    shutdown.cancel();
    drop(frames);
    // Closing publication lets lighting drain independently while native cleanup runs.
    drop(publication);
    if let Some(completed) = completed {
        completed.cancelled().await;
    }
    if let Some(cleanup) = cleanup {
        cleanup.await.context("file cleanup task panicked")??;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{stream, stream::BoxStream};
    use prysm_core::{Frame, LinearColor, PixelFormat};
    use std::{io::ErrorKind, time::Duration};

    fn config() -> PipelineConfig {
        let config: prysm_contract::Config =
            toml::from_str(include_str!("../../../../prysm.toml")).unwrap();
        let mut config = PipelineConfig::from(&config);
        config.processing.smoothing_seconds = 0.0.try_into().unwrap();
        config.processing.remove_black_bars = false;
        config.processing.brightness_percent = 100.try_into().unwrap();
        config
    }
    fn input(frames: BoxStream<'static, Result<Frame>>) -> InputStream {
        InputStream {
            frames,
            completed: None,
            cleanup: None,
            availability: None,
        }
    }
    fn status() -> watch::Sender<PipelineStatus> {
        watch::channel(PipelineStatus {
            phase: PipelinePhase::Starting,
            applied_revision: Some(ConfigRevision::new(1)),
            ..super::super::runtime::idle()
        })
        .0
    }
    #[tokio::test]
    async fn lighting_publication_does_not_retain_source_pixels() {
        let mut pool = prysm_core::FramePool::default();
        let mut writer = pool.acquire(4, 2, PixelFormat::RGB24).unwrap();
        writer.as_mut_slice().fill(255);
        let frame = writer.finish();
        let pixels = frame.as_slice().as_ptr();
        let (publish, frames) = watch::channel(None);
        process(
            config(),
            input(stream::once(async move { Ok(frame) }).boxed()),
            mpsc::channel(8).1,
            publish,
            CancellationToken::new(),
            status(),
            None,
        )
        .await
        .unwrap();
        // An unread output still owns its colors, but the source allocation is reusable.
        assert!(frames.borrow().is_some());
        let writer = pool.acquire(4, 2, PixelFormat::RGB24).unwrap();
        assert_eq!(writer.finish().as_slice().as_ptr(), pixels);
    }
    #[tokio::test]
    async fn eof_closes_publication_and_output_drains_final_unseen_result() {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut output = PreparedOutput {
            backend: WledRenderer::new(socket.local_addr().unwrap()).unwrap(),
            leds: [1; 4],
        };
        let (publish, frames) = watch::channel(None);
        let (_commands, commands) = mpsc::channel(8);
        let config = config();
        let result = process(
            config,
            input(
                stream::once(async { Ok(Frame::fill(255, 4, 2, PixelFormat::RGB24).unwrap()) })
                    .boxed(),
            ),
            commands,
            publish,
            CancellationToken::new(),
            status(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(result, PipelineExit::Completed);
        consume(&mut output, frames).await.unwrap();
        let mut packet = [0; 1500];
        let size = tokio::time::timeout(Duration::from_secs(1), socket.recv(&mut packet))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&packet[10..size], &[255; 12]);
        assert_eq!(
            socket.try_recv(&mut packet).unwrap_err().kind(),
            ErrorKind::WouldBlock
        );
    }
    #[tokio::test]
    async fn continuously_ready_input_gives_commands_and_cancellation_progress() {
        let (publish, mut frames) = watch::channel(None);
        let (commands, receiver) = mpsc::channel(8);
        let shutdown = CancellationToken::new();
        let stopped = shutdown.clone();
        let status = status();
        let observed = status.clone();
        let processing = tokio::spawn(process(
            config(),
            input(
                stream::repeat_with(|| Ok(Frame::fill(255, 4, 2, PixelFormat::RGB24).unwrap()))
                    .boxed(),
            ),
            receiver,
            publish,
            shutdown,
            status,
            None,
        ));
        frames.changed().await.unwrap();
        let mut settings = config().processing;
        settings.brightness_percent = 0.try_into().unwrap();
        let (reply, applied) = oneshot::channel();
        commands
            .send(Configure {
                settings,
                revision: ConfigRevision::new(8),
                reply,
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), applied)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            observed.borrow().applied_revision,
            Some(ConfigRevision::new(8))
        );
        loop {
            frames.changed().await.unwrap();
            let black = frames
                .borrow_and_update()
                .as_ref()
                .unwrap()
                .top
                .sample_at(0.5)
                == LinearColor::black();
            if black {
                break;
            }
        }
        stopped.cancel();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), processing)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            PipelineExit::Stopped
        );
    }
    #[tokio::test]
    async fn configuration_does_not_cancel_in_flight_source_read() {
        let (prefix_read, prefix) = oneshot::channel();
        let (continue_read, suffix) = oneshot::channel();
        let stream = stream::once(async {
            prefix_read.send(()).unwrap();
            suffix.await.unwrap();
            Ok(Frame::fill(255, 4, 2, PixelFormat::RGB24).unwrap())
        })
        .boxed();
        let (publish, mut frames) = watch::channel(None);
        let (commands, receiver) = mpsc::channel(8);
        let processing = tokio::spawn(process(
            config(),
            input(stream),
            receiver,
            publish,
            CancellationToken::new(),
            status(),
            None,
        ));
        prefix.await.unwrap();
        let (reply, applied) = oneshot::channel();
        commands
            .send(Configure {
                settings: config().processing,
                revision: ConfigRevision::new(3),
                reply,
            })
            .await
            .unwrap();
        applied.await.unwrap().unwrap();
        continue_read.send(()).unwrap();
        frames.changed().await.unwrap();
        assert_eq!(
            frames
                .borrow()
                .as_ref()
                .unwrap()
                .top
                .sample_at(0.5)
                .to_srgb(),
            prysm_core::Color {
                r: 255,
                g: 255,
                b: 255
            }
        );
        assert_eq!(processing.await.unwrap().unwrap(), PipelineExit::Completed);
    }
    #[tokio::test]
    async fn cancellation_keeps_runtime_driven_until_native_release() {
        let completed = CancellationToken::new();
        let released = completed.clone();
        let (release, gate) = oneshot::channel();
        let monitor = tokio::spawn(async move {
            gate.await.unwrap();
            released.cancel();
        });
        let mut source = input(stream::pending().boxed());
        source.completed = Some(completed.clone());
        let shutdown = CancellationToken::new();
        shutdown.cancel();
        let processing = process(
            config(),
            source,
            mpsc::channel(8).1,
            watch::channel(None).0,
            shutdown,
            status(),
            None,
        );
        tokio::pin!(processing);
        assert!(futures::poll!(&mut processing).is_pending());
        assert!(!completed.is_cancelled());
        release.send(()).unwrap();
        assert_eq!(processing.await.unwrap(), PipelineExit::Stopped);
        monitor.await.unwrap();
    }
    #[tokio::test]
    async fn output_failure_retains_context_and_cancels_capture() {
        let mut output = PreparedOutput {
            backend: WledRenderer::new("255.255.255.255:4048").unwrap(),
            leds: [1; 4],
        };
        let (publish, frames) = watch::channel(None);
        publish.send_replace(Some(EdgeColors::default()));
        drop(publish);
        assert!(
            consume(&mut output, frames)
                .await
                .unwrap_err()
                .to_string()
                .contains("cannot send WLED frame")
        );
    }
    #[test]
    #[ignore = "local WLED scheduling measurement; run with --ignored --nocapture"]
    fn measure_synchronous_wled_handler() {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_nonblocking(true).unwrap();
        let mut renderer = WledRenderer::new(socket.local_addr().unwrap()).unwrap();
        let colors = prysm_core::EdgeColors::fill(
            LinearColor::new(0.2, 0.4, 0.8),
            1920,
            1080,
            prysm_core::SampleDensity(50),
        );
        for count in [300, 65535] {
            let mut timings = Vec::new();
            for _ in 0..100 {
                let start = std::time::Instant::now();
                renderer.render_edges(&colors, [count, 0, 0, 0]).unwrap();
                timings.push(start.elapsed());
                while socket.recv(&mut [0; 1500]).is_ok() {}
            }
            timings.sort_unstable();
            println!(
                "WLED synchronous resample/pack/nonblocking loopback UDP: {count} LEDs p50={:?} p99={:?} max={:?}",
                timings[50], timings[98], timings[99]
            );
        }
    }
    #[tokio::test]
    #[ignore = "local synthetic delivery/age measurement; run with --ignored --nocapture"]
    async fn measure_processing_delivery_and_local_age() {
        use std::{
            collections::HashMap,
            sync::{Arc, Mutex},
            time::Instant,
        };
        const COUNT: usize = 240;
        let originals: Vec<_> = (0..COUNT)
            .map(|i| Frame::fill(u8::try_from(i).unwrap(), 320, 180, PixelFormat::RGB24).unwrap())
            .collect();
        let timestamps = Arc::new(Mutex::new(HashMap::<usize, Instant>::new()));
        let recorded = timestamps.clone();
        let (gate, started) = oneshot::channel();
        let mut gate = Some(gate);
        let mut polled = 0;
        let source = stream::iter(originals.clone().into_iter().map(Ok))
            .inspect(move |frame| {
                recorded.lock().unwrap().insert(
                    usize::from(frame.as_ref().unwrap().as_slice()[0]),
                    Instant::now(),
                );
                polled += 1;
                if polled == 32 {
                    let _ = gate.take().unwrap().send(());
                }
            })
            .boxed();
        let (publish, mut frames) = watch::channel(None);
        let status = status();
        let observed = status.clone();
        let (_commands, commands) = mpsc::channel(8);
        let (finished, done) = oneshot::channel();
        let worker = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let mut configuration = config();
            configuration.processing.shadow_lift_percent = 0.try_into().unwrap();
            let result = runtime.block_on(process(
                configuration,
                input(source),
                commands,
                publish,
                CancellationToken::new(),
                status,
                None,
            ));
            let _ = finished.send(result);
        });
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_nonblocking(true).unwrap();
        let mut output = PreparedOutput {
            backend: WledRenderer::new(socket.local_addr().unwrap()).unwrap(),
            leds: [100, 50, 100, 50],
        };
        // Deliberately withhold the observer until 32 ready frames: latest-value delivery
        // must coalesce them instead of accumulating a queue.
        started.await.unwrap();
        let mut ages = Vec::new();
        while frames.changed().await.is_ok() {
            let snapshot = frames.borrow_and_update().clone().unwrap();
            output.render(&snapshot).unwrap();
            let started =
                timestamps.lock().unwrap()[&usize::from(snapshot.top.sample_at(0.0).to_srgb().r)];
            ages.push(started.elapsed());
            while socket.recv(&mut [0; 1500]).is_ok() {}
            tokio::task::yield_now().await;
        }
        assert_eq!(done.await.unwrap().unwrap(), PipelineExit::Completed);
        tokio::task::spawn_blocking(move || worker.join())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(observed.borrow().frames_processed, COUNT as u64);
        assert!(!ages.is_empty() && ages.len() <= COUNT - 30);
        let delivered = ages.len();
        ages.sort_unstable();
        println!(
            "Synthetic ready-source320x180,300LED loopback: processed={COUNT}, output deliveries={delivered}, latest-watch coalesced={}; local source-poll→WLED-render-return age p50={:?} p99={:?} max={:?}; initial32-frame observer gate intentionally stresses coalescing. Excludes source acquisition, hardware/network delivery and browser latency.",
            COUNT - delivered,
            ages[delivered / 2],
            ages[(delivered - 1) * 99 / 100],
            ages[delivered - 1]
        );
        drop(originals);
    }
}
