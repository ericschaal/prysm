use anyhow::{Result, ensure};
use futures::{Stream, StreamExt};
use prysm_capture::Frame;
use prysm_core::EdgeColors;
use prysm_processor::PrysmProcessor;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Own processing and desktop publication for the lifetime of a frame source.
pub(crate) async fn publish_frames(
    source: impl Stream<Item = Frame>,
    mut processor: PrysmProcessor,
    frames: watch::Sender<Frame>,
    edge_colors: watch::Sender<EdgeColors>,
    shutdown: &CancellationToken,
) -> Result<()> {
    let _shutdown_guard = shutdown.clone().drop_guard();
    futures::pin_mut!(source);
    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => return Ok(()),
            frame = source.next() => {
                let Some(frame) = frame else {
                    ensure!(shutdown.is_cancelled(), "Capture stream ended unexpectedly");
                    return Ok(());
                };
                let colors = processor.process_frame(frame.clone());
                let _ = frames.send(frame);
                let _ = edge_colors.send(colors);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_capture::PixelFormat;
    use prysm_core::Config;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    struct DropProbe(Arc<AtomicBool>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    fn outputs() -> (watch::Sender<Frame>, watch::Sender<EdgeColors>) {
        let (frames, _) = watch::channel(Frame::fill(0, 2, 2, PixelFormat::RGB24));
        let (colors, _) = watch::channel(EdgeColors::default());
        (frames, colors)
    }

    #[tokio::test]
    async fn publishes_every_frame_in_a_burst_and_reports_unexpected_eof() {
        let (frames, colors) = outputs();
        let frame_rx = frames.subscribe();
        let color_rx = colors.subscribe();
        let source = futures::stream::iter(1..=8).map(|value| {
            if value > 1 {
                assert_eq!(frame_rx.borrow().as_slice(), &[value - 1; 12]);
                assert_eq!(color_rx.borrow().top.sample_at(0.5).to_srgb().r, value - 1);
            }
            Frame::fill(value, 2, 2, PixelFormat::RGB24)
        });
        let config = Config {
            brightness_percent: 100,
            smoothing_seconds: 0.0,
            remove_black_bars: false,
            ..Config::default()
        };
        let shutdown = CancellationToken::new();
        let result = publish_frames(
            source,
            PrysmProcessor::new(&config),
            frames,
            colors,
            &shutdown,
        )
        .await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Capture stream ended unexpectedly")
        );
        assert!(shutdown.is_cancelled());
        assert_eq!(frame_rx.borrow().as_slice(), &[8; 12]);
        assert_eq!(color_rx.borrow().top.sample_at(0.5).to_srgb().r, 8);
        assert!(frame_rx.has_changed().is_err());
        assert!(color_rx.has_changed().is_err());
    }

    #[tokio::test]
    async fn cancellation_drops_a_pending_source_and_closes_outputs() {
        let dropped = Arc::new(AtomicBool::new(false));
        let probe = DropProbe(dropped.clone());
        let source = futures::stream::poll_fn(move |_| {
            let _ = &probe;
            std::task::Poll::Pending
        });
        let (frames, colors) = outputs();
        let frame_rx = frames.subscribe();
        let color_rx = colors.subscribe();
        let shutdown = CancellationToken::new();
        let mut publishing = Box::pin(publish_frames(
            source,
            PrysmProcessor::default(),
            frames,
            colors,
            &shutdown,
        ));
        assert!(futures::poll!(&mut publishing).is_pending());
        shutdown.cancel();
        publishing.await.unwrap();
        assert!(dropped.load(Ordering::SeqCst));
        assert!(frame_rx.has_changed().is_err());
        assert!(color_rx.has_changed().is_err());
    }

    #[tokio::test]
    async fn cancellation_during_eof_poll_is_successful() {
        let shutdown = CancellationToken::new();
        let source = futures::stream::poll_fn(|_| {
            shutdown.cancel();
            std::task::Poll::Ready(None)
        });
        let (frames, colors) = outputs();
        publish_frames(source, PrysmProcessor::default(), frames, colors, &shutdown)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn pipeline_panic_is_observed_and_cancels_shutdown() {
        let shutdown = CancellationToken::new();
        let token = shutdown.clone();
        let dropped = Arc::new(AtomicBool::new(false));
        let probe = DropProbe(dropped.clone());
        let task = tokio::spawn(async move {
            let source = futures::stream::poll_fn(move |_| -> std::task::Poll<Option<Frame>> {
                let _ = &probe;
                panic!("pipeline failed");
            });
            let (frames, colors) = outputs();
            publish_frames(source, PrysmProcessor::default(), frames, colors, &token).await
        });
        assert!(task.await.unwrap_err().is_panic());
        assert!(shutdown.is_cancelled());
        assert!(dropped.load(Ordering::SeqCst));
    }
}
