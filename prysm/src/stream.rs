use futures::{Stream, StreamExt};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::BroadcastStream;

pub async fn wait_for_shutdown(
    shutdown: &tokio_util::sync::CancellationToken,
    mut frame_task: JoinHandle<()>,
) -> anyhow::Result<()> {
    use anyhow::Context;
    tokio::select! {
        biased;
        () = shutdown.cancelled() => frame_task.await.context("Frame watcher failed"),
        result = &mut frame_task => {
            let unexpected = !shutdown.is_cancelled();
            shutdown.cancel();
            result.context("Frame watcher failed")?;
            anyhow::ensure!(!unexpected, "Capture stream ended unexpectedly");
            Ok(())
        }
    }
}

pub fn stream_split<S>(source: S) -> (impl Stream<Item = S::Item>, impl Stream<Item = S::Item>)
where
    S: Stream + Send + 'static,
    S::Item: Clone + Send,
{
    // Create broadcast channel for frame distribution
    let (frame_tx, processor_rx) = broadcast::channel::<S::Item>(3);

    // Subscribe to broadcast for renderer
    let renderer_rx = frame_tx.subscribe();

    // Spawn task to broadcast frames
    tokio::spawn(async move {
        futures::pin_mut!(source);
        while let Some(frame) = source.next().await {
            let _ = frame_tx.send(frame);
        }
    });

    // Convert broadcast receiver to stream for processor
    let processor_stream =
        BroadcastStream::new(processor_rx).filter_map(|result| async move { result.ok() });
    let renderer_stream =
        BroadcastStream::new(renderer_rx).filter_map(|result| async move { result.ok() });

    (processor_stream, renderer_stream)
}

#[derive(Debug, Clone)]
pub struct StreamWatcher<T: Clone + Send + Sync + 'static> {
    tx: tokio::sync::watch::Sender<T>,
    rx: tokio::sync::watch::Receiver<T>,
}

impl<T: Clone + Send + Sync + 'static> StreamWatcher<T> {
    #[must_use]
    pub fn new(init: T) -> StreamWatcher<T> {
        let (tx, rx) = tokio::sync::watch::channel(init);
        StreamWatcher { tx, rx }
    }
    pub fn into_task<S>(self, stream: S) -> JoinHandle<()>
    where
        S: Stream<Item = T> + Send + 'static,
    {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            futures::pin_mut!(stream);
            while let Some(item) = stream.next().await {
                let _ = tx.send(item);
            }
        })
    }

    pub fn receiver(&self) -> tokio::sync::watch::Receiver<T> {
        self.rx.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn ended_capture_closes_both_consumers_and_reports_failure() {
        let shutdown = CancellationToken::new();
        let (a, b) = stream_split(futures::stream::iter([1u8]));
        let spectra = StreamWatcher::new(0).into_task(a);
        let frames = StreamWatcher::new(0).into_task(b);
        let result = wait_for_shutdown(&shutdown, frames).await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Capture stream ended unexpectedly")
        );
        assert!(shutdown.is_cancelled());
        spectra.await.unwrap();
    }

    #[tokio::test]
    async fn requested_shutdown_is_successful() {
        let shutdown = CancellationToken::new();
        let token = shutdown.clone();
        let frames = tokio::spawn(async move {
            token.cancelled().await;
        });
        shutdown.cancel();
        wait_for_shutdown(&shutdown, frames).await.unwrap();
    }
}
