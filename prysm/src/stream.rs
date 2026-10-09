use futures::{Stream, StreamExt};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::BroadcastStream;

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
