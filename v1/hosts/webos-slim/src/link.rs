//! The forwarder's side of the supervisor link, and the counting sink that
//! tells it whether frames flow.
use crate::video::VideoFeed;
use futures::future::BoxFuture;
use prysm_core::Frame;
use prysm_ports::{FrameSink, Result};
use prysm_runtime::{Shutdown, Surface};
use prysm_webos_link::worker::{Link, Stop};
use prysm_webos_link::{Command, Event, HEARTBEAT, WorkerStatus};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::time::{Instant, sleep_until};

/// How often frame flow is sampled; `capturing` means frames in the last window.
const SAMPLE: Duration = Duration::from_secs(1);

/// A [`FrameSink`] decorator counting frames that were sent.
pub struct Counted<S> {
    inner: S,
    sent: Arc<AtomicU64>,
}

impl<S: FrameSink> Counted<S> {
    pub fn new(inner: S) -> (Self, Arc<AtomicU64>) {
        let sent = Arc::new(AtomicU64::new(0));
        (
            Self {
                inner,
                sent: sent.clone(),
            },
            sent,
        )
    }
}

impl<S: FrameSink> FrameSink for Counted<S> {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        self.inner.send(frame)?;
        self.sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// `Surface<()>`: video commands into the capture, frame flow out as status.
/// `Lighting` is protocol misuse here and stops the worker as failed.
pub struct ForwarderLink<V> {
    link: Link,
    video: V,
    sent: Arc<AtomicU64>,
    sample: Duration,
    heartbeat: Duration,
}

impl<V: VideoFeed> ForwarderLink<V> {
    pub fn new(link: Link, video: V, sent: Arc<AtomicU64>) -> Self {
        Self {
            link,
            video,
            sent,
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
        let mut counted = self.sent.load(Ordering::Relaxed);
        let mut status = WorkerStatus::default();
        events.send(&Event::Status(status.clone()));
        let mut beat = Instant::now() + self.heartbeat;
        let mut sample = Instant::now() + self.sample;
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
                    sample = Instant::now() + self.sample;
                    let now = self.sent.load(Ordering::Relaxed);
                    let capturing = now != counted;
                    counted = now;
                    if capturing != status.capturing {
                        status.capturing = capturing;
                        events.send(&Event::Status(status.clone()));
                        beat = Instant::now() + self.heartbeat;
                    }
                }
                () = sleep_until(beat) => {
                    events.send(&Event::Status(status.clone()));
                    beat = Instant::now() + self.heartbeat;
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
