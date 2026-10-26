//! Compose a capture session on its own Actix thread.
use super::{
    RenderFrame, Stop,
    capturer::Capturer,
    processor::{ConfigureProcessor, Processor},
    wled::Wled,
};
use crate::config::PipelineConfig;
use actix::{Actor, Arbiter, Recipient};
use anyhow::{Context as _, Result, anyhow};
use prysm_contract::{Output, PipelineStatus};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

const STOP_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineExit {
    Completed,
    Stopped,
}

/// Terminal notifications to the async caller. Frame traffic uses Actix recipients.
pub type Completion = mpsc::Sender<Result<PipelineExit>>;
type Ready = oneshot::Sender<Result<Recipient<ConfigureProcessor>>>;

pub(super) struct RenderTarget {
    frames: Recipient<RenderFrame>,
    stop: Recipient<Stop>,
}

/// Available outputs; the native window supplies its watch channels at startup.
#[derive(Clone, Default)]
pub(crate) struct Renderers {
    #[cfg(feature = "desktop")]
    pub egui: Option<super::egui::Egui>,
}

impl Renderers {
    fn create(&self, output: &Output, completion: Completion) -> Result<RenderTarget> {
        match output {
            Output::Wled {
                address,
                refresh_rate_hz,
                leds,
            } => {
                let renderer = Wled::new(address, *refresh_rate_hz, *leds, completion)?.start();
                Ok(RenderTarget {
                    frames: renderer.clone().recipient(),
                    stop: renderer.recipient(),
                })
            }
            Output::Egui { .. } => {
                #[cfg(feature = "desktop")]
                {
                    let renderer = self
                        .egui
                        .clone()
                        .context("restart the application to open the egui window")?
                        .start();
                    Ok(RenderTarget {
                        frames: renderer.clone().recipient(),
                        stop: renderer.recipient(),
                    })
                }
                #[cfg(not(feature = "desktop"))]
                anyhow::bail!("egui output requires building with --features desktop")
            }
        }
    }
}

pub(super) struct Session {
    arbiter: Option<Arbiter>,
    pub shutdown: CancellationToken,
    pub processor: Recipient<ConfigureProcessor>,
    completion: oneshot::Receiver<Result<PipelineExit>>,
}

impl Session {
    pub async fn wait(mut self) -> Result<PipelineExit> {
        let result = (&mut self.completion)
            .await
            .context("pipeline thread ended unexpectedly");
        let arbiter = self.arbiter.take().expect("session owns its thread");
        join(arbiter).await?;
        result?
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

async fn join(arbiter: Arbiter) -> Result<()> {
    arbiter.stop();
    tokio::task::spawn_blocking(move || arbiter.join())
        .await
        .context("cannot join pipeline thread")?
        .map_err(|_| anyhow!("pipeline thread panicked"))
}

/// Open input and wire actors on their thread, returning only when setup has succeeded.
pub(super) async fn start(
    config: PipelineConfig,
    status: watch::Sender<PipelineStatus>,
    renderers: Renderers,
) -> Result<Session> {
    start_with_output(config, status, move |output, completion| {
        renderers.create(output, completion)
    })
    .await
}

async fn start_with_output<F>(
    config: PipelineConfig,
    status: watch::Sender<PipelineStatus>,
    create_output: F,
) -> Result<Session>
where
    F: FnOnce(&Output, Completion) -> Result<RenderTarget> + Send + 'static,
{
    let arbiter = Arbiter::new();
    let shutdown = CancellationToken::new();
    let cancel_on_drop = shutdown.clone().drop_guard();
    let worker_shutdown = shutdown.clone();
    let (ready_tx, ready_rx) = oneshot::channel();
    let (finished, completion) = oneshot::channel();
    arbiter.spawn(async move {
        let mut ready = Some(ready_tx);
        let result = async {
            let frames = crate::input::into_stream(
                config.input.clone(),
                config.resolution.to_core(),
                worker_shutdown.clone(),
            )?;
            run_session(
                config,
                frames,
                worker_shutdown,
                status,
                &mut ready,
                create_output,
            )
            .await
        }
        .await;
        if let Some(ready) = ready {
            let error = result
                .err()
                .unwrap_or_else(|| anyhow!("pipeline ended before startup"));
            let _ = ready.send(Err(error));
        } else {
            let _ = finished.send(result);
        }
        Arbiter::current().stop();
    });
    match ready_rx
        .await
        .context("pipeline thread ended during startup")
        .and_then(|result| result)
    {
        Ok(processor) => {
            cancel_on_drop.disarm();
            Ok(Session {
                arbiter: Some(arbiter),
                shutdown,
                processor,
                completion,
            })
        }
        Err(error) => {
            join(arbiter).await?;
            Err(error)
        }
    }
}

async fn run_session<F>(
    config: PipelineConfig,
    input: crate::input::InputStream,
    shutdown: CancellationToken,
    status: watch::Sender<PipelineStatus>,
    ready: &mut Option<Ready>,
    create_output: F,
) -> Result<PipelineExit>
where
    F: FnOnce(&Output, Completion) -> Result<RenderTarget>,
{
    let result = async {
        // Each actor reports at most one terminal result.
        let (completion, mut outcomes) = mpsc::channel(3);
        let renderer = create_output(&config.output, completion.clone())?;
        let processor = Processor::new(
            &config.processing,
            renderer.frames,
            completion.clone(),
            status,
        )
        .start();
        let capturer = Capturer::spawn(input.frames, processor.clone().recipient(), completion);
        if let Some(ready) = ready.take() {
            let _ = ready.send(Ok(processor.clone().recipient()));
        }
        let outcome = tokio::select! {
            biased;
            () = shutdown.cancelled() => Ok(PipelineExit::Stopped),
            outcome = outcomes.recv() => outcome.context("pipeline actors ended unexpectedly")?,
        };
        // Stop the producer before FIFO barriers drain accepted processing and output.
        let drain = tokio::time::timeout(STOP_TIMEOUT, async {
            if capturer.connected() {
                let _ = capturer.send(Stop).await;
            }
            let processor_stop = processor.send(Stop).await;
            let renderer_stop = renderer.stop.send(Stop).await;
            processor_stop.context("processor stopped while draining")?;
            renderer_stop.context("renderer stopped while draining")?;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("pipeline shutdown timed out")?;
        let mut outcome = outcome;
        while let Ok(event) = outcomes.try_recv() {
            if event.is_err() && outcome.is_ok() {
                outcome = event;
            }
        }
        outcome.and_then(|exit| drain.map(|()| exit))
    }
    .await;
    shutdown.cancel();
    // Keep this runtime alive until the camera worker releases the device.
    if let Some(completed) = input.completed {
        completed.cancelled().await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{StreamExt, stream, stream::BoxStream};
    use prysm_capture::{Frame, PixelFormat};
    use prysm_contract::{ConfigRevision, ProcessingSettings};
    use std::io::ErrorKind;

    async fn test_session(
        config: PipelineConfig,
        frames: BoxStream<'static, Result<Frame>>,
        shutdown: CancellationToken,
    ) -> Result<PipelineExit> {
        let (status, _) = watch::channel(super::super::runtime::idle());
        run_session(
            config,
            crate::input::InputStream {
                frames,
                completed: None,
            },
            shutdown,
            status,
            &mut None,
            |output, completion| Renderers::default().create(output, completion),
        )
        .await
    }

    fn config(address: String) -> PipelineConfig {
        PipelineConfig {
            revision: ConfigRevision::default(),
            input: prysm_contract::Input::File {
                path: "/missing/prysm-pipeline-test.mp4"
                    .to_owned()
                    .try_into()
                    .unwrap(),
            },
            resolution: [4, 4].try_into().unwrap(),
            processing: ProcessingSettings {
                smoothing_seconds: 0.0.try_into().unwrap(),
                remove_black_bars: false,
                ..ProcessingSettings::default()
            },
            output: serde_json::from_value(serde_json::json!({
                "type": "wled",
                "address": address,
                "refresh_rate_hz": 60,
                "leds": {"top": 1, "right": 1, "bottom": 1, "left": 1}
            }))
            .unwrap(),
        }
    }

    #[cfg(feature = "desktop")]
    #[actix::test]
    async fn egui_output_drains_the_final_frame_without_wled() {
        use super::super::egui::Egui;
        use prysm_core::{EdgeColors, LinearColor};
        use prysm_renderer_desktop::DisplayFrame;

        let (colors, colors_rx) = watch::channel(EdgeColors::default());
        let (frames, frames_rx) = watch::channel(DisplayFrame {
            frame: Frame::dummy(4, 4),
            viewport: None,
        });
        let renderers = Renderers {
            egui: Some(Egui { colors, frames }),
        };
        let mut config = config("127.0.0.1:4048".into());
        config.output = Output::Egui {
            refresh_rate_hz: 30.try_into().unwrap(),
        };
        let exit = run_session(
            config,
            crate::input::InputStream {
                frames: stream::once(async { Ok(Frame::fill(255, 4, 4, PixelFormat::RGB24)) })
                    .boxed(),
                completed: None,
            },
            CancellationToken::new(),
            watch::channel(super::super::runtime::idle()).0,
            &mut None,
            move |output, completion| renderers.create(output, completion),
        )
        .await
        .unwrap();
        assert_eq!(exit, PipelineExit::Completed);
        assert_eq!(frames_rx.borrow().frame.as_slice(), &[255; 48]);
        assert_eq!(
            colors_rx.borrow().top.sample_at(0.5),
            LinearColor::new(1.0, 1.0, 1.0)
        );
    }

    #[actix::test]
    async fn eof_drains_the_final_accepted_frame() {
        let receiver = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let config = config(receiver.local_addr().unwrap().to_string());
        let frames = stream::once(async { Ok(Frame::fill(255, 4, 4, PixelFormat::RGB24)) });

        let exit = test_session(config, frames.boxed(), CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(exit, PipelineExit::Completed);
        let mut packet = [0; 1500];
        let size = tokio::time::timeout(Duration::from_secs(1), receiver.recv(&mut packet))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(size, 22);
        assert_eq!(&packet[10..size], &[255; 12]);
        assert_eq!(
            receiver.try_recv(&mut packet).unwrap_err().kind(),
            ErrorKind::WouldBlock
        );
    }

    #[actix::test]
    async fn cancellation_drops_input_and_returns_stopped() {
        let receiver = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let config = config(receiver.local_addr().unwrap().to_string());
        let (resource, dropped) = oneshot::channel::<()>();
        let frames = stream::once(async move {
            let _resource = resource;
            futures::future::pending::<Result<Frame>>().await
        });
        let shutdown = CancellationToken::new();
        shutdown.cancel();

        assert_eq!(
            test_session(config, frames.boxed(), shutdown)
                .await
                .unwrap(),
            PipelineExit::Stopped
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(1), dropped)
                .await
                .unwrap()
                .is_err()
        );
        assert_eq!(
            receiver.try_recv(&mut [0; 1500]).unwrap_err().kind(),
            ErrorKind::WouldBlock
        );
    }

    #[actix::test]
    async fn shutdown_and_startup_failure_wait_for_camera_release() {
        for fail_output in [false, true] {
            let receiver = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let config = config(receiver.local_addr().unwrap().to_string());
            let completed = CancellationToken::new();
            let (release, gate) = std::sync::mpsc::channel();
            let (resource, mut dropped) = oneshot::channel::<()>();
            let worker_completed = completed.clone();
            let worker = std::thread::spawn(move || {
                let _completed = worker_completed.drop_guard();
                let _resource = resource;
                gate.recv().unwrap();
            });
            let shutdown = CancellationToken::new();
            shutdown.cancel();
            let mut ready = None;
            let session = run_session(
                config,
                crate::input::InputStream {
                    frames: stream::pending().boxed(),
                    completed: Some(completed),
                },
                shutdown,
                watch::channel(super::super::runtime::idle()).0,
                &mut ready,
                move |output, completion| {
                    anyhow::ensure!(!fail_output, "test output startup failure");
                    Renderers::default().create(output, completion)
                },
            );
            futures::pin_mut!(session);
            assert!(futures::poll!(&mut session).is_pending());
            assert!(matches!(
                dropped.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ));
            release.send(()).unwrap();
            let result = tokio::time::timeout(Duration::from_secs(2), session)
                .await
                .unwrap();
            if fail_output {
                assert_eq!(
                    result.unwrap_err().to_string(),
                    "test output startup failure"
                );
            } else {
                assert_eq!(result.unwrap(), PipelineExit::Stopped);
            }
            assert!(
                dropped.await.is_err(),
                "device must be released before returning"
            );
            worker.join().unwrap();
        }
    }

    #[actix::test]
    async fn capture_failure_is_reported() {
        let receiver = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let config = config(receiver.local_addr().unwrap().to_string());
        let frames = stream::once(async {
            Err(std::io::Error::new(ErrorKind::NotConnected, "test capture failure").into())
        });

        let error = test_session(config, frames.boxed(), CancellationToken::new())
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "test capture failure");
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            ErrorKind::NotConnected
        );
    }

    #[actix::test]
    async fn final_render_failure_takes_precedence_over_eof() {
        // Broadcast sends fail without SO_BROADCAST; no packet leaves the socket.
        let config = config("255.255.255.255:4048".into());
        let frames = stream::once(async { Ok(Frame::dummy(4, 4)) });

        let error = test_session(config, frames.boxed(), CancellationToken::new())
            .await
            .unwrap_err();

        assert!(
            error.to_string().contains("cannot send WLED frame"),
            "{error:#}"
        );
    }

    #[actix::test]
    async fn startup_failure_returns_from_the_dedicated_thread() {
        let config = config("127.0.0.1:4048".into());
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            start(
                config,
                watch::channel(super::super::runtime::idle()).0,
                Renderers::default(),
            ),
        )
        .await
        .unwrap()
        .err()
        .expect("startup must fail");

        assert!(error.to_string().contains("Cannot open video"), "{error:#}");
    }
}
