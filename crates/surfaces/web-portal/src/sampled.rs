//! Demand-driven SSE for runtime feeds. Each body samples the newest value when
//! the previous write is done. Idle clients hold one slot until they disconnect.
use crate::response::{self, KEEPALIVE, KEEPALIVE_COMMENT, Response, failure};
use bytes::Bytes;
use futures::stream;
use hyper::body::Frame;
use prysm_protocol::Failure;
use prysm_runtime::{Shutdown, Watch};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::time::{Instant, sleep_until};

/// What makes a sample distinct and how the endpoint puts it on the wire.
pub(crate) trait Events: Send + Sync + 'static {
    type Sample: Clone + Send + Sync;
    type Key: Eq + Send + Sync;

    fn key(&self, value: &Self::Sample) -> Self::Key;
    fn idle(&self) -> Bytes;
    fn encode(&self, value: Self::Sample, key: &Self::Key) -> impl Future<Output = Bytes> + Send;
}

pub(crate) struct Sampled<E> {
    slots: Arc<Semaphore>,
    busy: &'static str,
    cadence: Duration,
    events: Arc<E>,
}

impl<E: Events> Sampled<E> {
    pub(crate) fn new(limit: usize, busy: &'static str, cadence: Duration, events: E) -> Self {
        Self {
            slots: Arc::new(Semaphore::new(limit)),
            busy,
            cadence,
            events: Arc::new(events),
        }
    }

    pub(crate) fn stream(
        &self,
        watch: impl FnOnce() -> Watch<E::Sample>,
        shutdown: &Shutdown,
    ) -> Response {
        let Ok(permit) = self.slots.clone().try_acquire_owned() else {
            return failure(Failure::Busy {
                message: self.busy.into(),
            });
        };
        let now = Instant::now();
        let viewer = Viewer {
            watch: watch(),
            events: self.events.clone(),
            shown: Shown::Starting,
            due: now,
            sent: now,
            cadence: self.cadence,
            _permit: permit,
        };
        response::event_stream(stream::unfold(viewer, Viewer::next), shutdown)
    }
}

enum Shown<K> {
    Starting,
    AwaitingFirst,
    Idle,
    Sample(K),
}

struct Viewer<E: Events> {
    watch: Watch<E::Sample>,
    events: Arc<E>,
    shown: Shown<E::Key>,
    cadence: Duration,
    due: Instant,
    sent: Instant,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl<E: Events> Viewer<E> {
    async fn next(mut self) -> Option<(Frame<Bytes>, Self)> {
        loop {
            sleep_until(self.due).await;
            self.due = Instant::now() + self.cadence;
            let event = match self.watch.latest() {
                None if matches!(self.shown, Shown::Starting) => {
                    // Opening created demand. Give the producer one period before reporting idle.
                    self.shown = Shown::AwaitingFirst;
                    continue;
                }
                None if matches!(self.shown, Shown::Idle) => None,
                None => {
                    self.shown = Shown::Idle;
                    Some(self.events.idle())
                }
                Some(value) => {
                    let key = self.events.key(&value);
                    if matches!(&self.shown, Shown::Sample(last) if *last == key) {
                        None
                    } else {
                        let event = self.events.encode(value, &key).await;
                        self.shown = Shown::Sample(key);
                        Some(event)
                    }
                }
            };
            let event = match event {
                Some(event) => event,
                None => tokio::select! {
                    () = self.watch.changed() => continue,
                    () = sleep_until(self.sent + KEEPALIVE) => KEEPALIVE_COMMENT,
                },
            };
            self.sent = Instant::now();
            return Some((Frame::data(event), self));
        }
    }
}
