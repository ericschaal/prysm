use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use futures::{Stream, StreamExt};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

/// State shared between two tee'd streams
struct TeeState<S, T> {
    source: Pin<Box<S>>,
    /// Item cached for the consumer that hasn't read it yet
    cache: Option<T>,
    /// Source stream is exhausted
    done: bool,
}

/// A stream that shares items with its sibling
struct TeeStream<S: Stream> {
    state: Arc<Mutex<TeeState<S, S::Item>>>,
}

impl<S> Stream for TeeStream<S>
where
    S: Stream,
    S::Item: Clone,
{
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut state = match self.state.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                // Other consumer is polling, wake us up to try again
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
        };

        // If we have a cached item, take it
        if let Some(item) = state.cache.take() {
            return Poll::Ready(Some(item));
        }

        // If source is done, we're done
        if state.done {
            return Poll::Ready(None);
        }

        // Poll the source stream
        match state.source.as_mut().poll_next(cx) {
            Poll::Ready(Some(item)) => {
                // Clone item: one for us, one for cache
                let item_clone = item.clone();
                state.cache = Some(item);
                Poll::Ready(Some(item_clone))
            }
            Poll::Ready(None) => {
                state.done = true;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Split a stream into two independent streams that both receive all items.
/// Items are cloned (cheap with Arc<Vec<u8>> in Frame).
pub fn stream_tee<S>(source: S) -> (impl Stream<Item = S::Item>, impl Stream<Item = S::Item>)
where
    S: Stream + 'static,
    S::Item: Clone,
{
    let state = Arc::new(Mutex::new(TeeState {
        source: Box::pin(source),
        cache: None,
        done: false,
    }));

    let stream1 = TeeStream {
        state: Arc::clone(&state),
    };

    let stream2 = TeeStream {
        state,
    };

    (stream1, stream2)
}

pub fn stream_split<S>(source: S) -> (impl Stream<Item = S::Item>, impl Stream<Item = S::Item>)
where
    S: Stream + Send + 'static,
    S::Item: Clone + Send,
{
    // Create broadcast channel for frame distribution
    let (frame_tx, processor_rx) = broadcast::channel::<S::Item>(10);

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
    let processor_stream = BroadcastStream::new(processor_rx)
        .filter_map(|result| async move { result.ok() });
    let renderer_stream = BroadcastStream::new(renderer_rx)
        .filter_map(|result| async move { result.ok() });

    (processor_stream, renderer_stream)
}
