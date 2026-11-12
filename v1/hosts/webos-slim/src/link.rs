//! The forwarder's side of the supervisor link, and the uplink sink that
//! keeps reconnecting to the receiver and tells the link whether frames flow.
use crate::video::VideoFeed;
use futures::future::BoxFuture;
use prysm_core::Frame;
use prysm_ports::{Error, FrameSink, Result};
use prysm_runtime::{Shutdown, Surface};
use prysm_webos_link::worker::{Link, Stop};
use prysm_webos_link::{Command, Event, HEARTBEAT, WorkerStatus};
use std::error::Error as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio::time::{Instant as TokioInstant, sleep_until};

/// How often frame flow is sampled; `capturing` means frames in the last window.
const SAMPLE: Duration = Duration::from_secs(1);

/// How long the uplink waits between connection attempts while the receiver
/// is unavailable (its host is not lighting, or unreachable).
pub const RECONNECT: Duration = Duration::from_secs(1);

/// What the uplink tells the link: frames delivered, and why it is waiting.
#[derive(Debug, Default)]
pub struct Flow {
    sent: AtomicU64,
    waiting: Mutex<Option<String>>,
}

impl Flow {
    fn sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }

    /// Why no frames can be delivered, while the receiver is unavailable.
    fn waiting(&self) -> Option<String> {
        self.lock().clone()
    }

    fn set_waiting(&self, reason: Option<String>) {
        *self.lock() = reason;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.waiting.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A [`FrameSink`] that delivers to whatever `connect` opens and keeps
/// reconnecting. The receiver only listens while its host is lighting, so a
/// refused connection or a dropped receiver is a normal wait, not a failure:
/// frames are dropped, a new connection is tried at most every [`RECONNECT`]
/// (on the next frame after it), and [`Flow`] carries the reason. `send`
/// never fails, so forwarding (and the worker) keep running.
pub struct Uplink<S, C> {
    connect: C,
    sink: Option<S>,
    retry_at: Option<Instant>,
    retry: Duration,
    flow: Arc<Flow>,
}

impl<S: FrameSink, C: FnMut() -> Result<S> + Send> Uplink<S, C> {
    pub fn new(connect: C) -> (Self, Arc<Flow>) {
        let flow = Arc::new(Flow::default());
        let uplink = Self {
            connect,
            sink: None,
            retry_at: None,
            retry: RECONNECT,
            flow: flow.clone(),
        };
        (uplink, flow)
    }

    #[cfg(test)]
    pub fn retry_every(mut self, retry: Duration) -> Self {
        self.retry = retry;
        self
    }

    /// Note the failure and wait [`RECONNECT`] before the next attempt.
    fn unavailable(&mut self, error: &Error) {
        self.sink = None;
        self.retry_at = Some(Instant::now() + self.retry);
        self.flow
            .set_waiting(Some(format!("waiting for the receiver: {}", chain(error))));
    }
}

impl<S: FrameSink, C: FnMut() -> Result<S> + Send> FrameSink for Uplink<S, C> {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        if self.sink.is_none() {
            if self.retry_at.is_some_and(|at| Instant::now() < at) {
                return Ok(());
            }
            match (self.connect)() {
                Ok(sink) => {
                    self.sink = Some(sink);
                    self.flow.set_waiting(None);
                }
                Err(error) => {
                    self.unavailable(&error);
                    return Ok(());
                }
            }
        }
        let Some(sink) = &mut self.sink else {
            return Ok(());
        };
        match sink.send(frame) {
            Ok(()) => {
                self.flow.sent.fetch_add(1, Ordering::Relaxed);
            }
            Err(error) => self.unavailable(&error),
        }
        Ok(())
    }
}

/// `error: cause: cause`, as the supervisor shows failures.
fn chain(error: &Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// `Surface<()>`: video commands into the capture, frame flow out as status:
/// `capturing` while frames are delivered, `failure` naming the wait while the
/// receiver is unavailable. `Lighting` is protocol misuse here and stops the
/// worker as failed.
pub struct ForwarderLink<V> {
    link: Link,
    video: V,
    flow: Arc<Flow>,
    sample: Duration,
    heartbeat: Duration,
}

impl<V: VideoFeed> ForwarderLink<V> {
    pub fn new(link: Link, video: V, flow: Arc<Flow>) -> Self {
        Self {
            link,
            video,
            flow,
            sample: SAMPLE,
            heartbeat: HEARTBEAT,
        }
    }

    #[cfg(test)]
    pub fn with_timing(mut self, sample: Duration, heartbeat: Duration) -> Self {
        self.sample = sample;
        self.heartbeat = heartbeat;
        self
    }

    async fn serve(self, shutdown: Shutdown) -> Result<()> {
        let Link {
            mut commands,
            events,
            stop,
        } = self.link;
        let mut counted = self.flow.sent();
        let mut status = WorkerStatus::default();
        events.send(&Event::Status(status.clone()));
        let mut beat = TokioInstant::now() + self.heartbeat;
        let mut sample = TokioInstant::now() + self.sample;
        loop {
            tokio::select! {
                () = shutdown.requested() => return Ok(()),
                command = commands.next() => match command {
                    None => return Ok(()),
                    Some(Command::Video { video }) => self.video.set(video),
                    Some(Command::ResetVideo {}) => self.video.reset(),
                    Some(Command::Lighting { .. }) => {
                        stop.request(Stop::Failed("`lighting` is not a slim worker command".into()));
                        return Ok(());
                    }
                },
                () = sleep_until(sample) => {
                    sample = TokioInstant::now() + self.sample;
                    let now = self.flow.sent();
                    let next = WorkerStatus {
                        capturing: now != counted,
                        failure: self.flow.waiting(),
                        ..WorkerStatus::default()
                    };
                    counted = now;
                    if next != status {
                        status = next;
                        events.send(&Event::Status(status.clone()));
                        beat = TokioInstant::now() + self.heartbeat;
                    }
                }
                () = sleep_until(beat) => {
                    events.send(&Event::Status(status.clone()));
                    beat = TokioInstant::now() + self.heartbeat;
                }
            }
        }
    }
}

impl<V: VideoFeed> Surface<()> for ForwarderLink<V> {
    fn run(self: Box<Self>, (): (), shutdown: Shutdown) -> BoxFuture<'static, Result<()>> {
        Box::pin(self.serve(shutdown))
    }
}
