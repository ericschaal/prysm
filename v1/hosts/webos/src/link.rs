//! The engine's side of the supervisor link: commands in, status out.
use crate::video::VideoFeed;
use futures::future::BoxFuture;
use prysm_ports::{Error, LightingTarget, Result, SessionState, Snapshot};
use prysm_runtime::{EngineHandle, Shutdown, Surface};
use prysm_webos_link::worker::{Link, Stop, chain};
use prysm_webos_link::{Command, Event, HEARTBEAT, WorkerStatus};
use std::time::Duration;
use tokio::time::{Instant, sleep_until};

/// Retry after a failed (not faulted) session: doubling from `RETRY_MIN`.
const RETRY_MIN: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(30);
/// A session that ran this long resets the retry delay.
const HEALTHY: Duration = Duration::from_secs(60);

/// `Surface<EngineHandle>`: applies the initial target, then turns link
/// commands into video signals and targets, reports every snapshot change
/// (and a heartbeat) as status, re-applies a non-Off target after a session
/// failure with backoff, and stops the worker on a faulted session.
pub struct EngineLink<V> {
    link: Link,
    video: V,
    initial: LightingTarget,
    portal: Option<String>,
    heartbeat: Duration,
    retry_min: Duration,
}

impl<V: VideoFeed> EngineLink<V> {
    pub fn new(link: Link, video: V, initial: LightingTarget, portal: Option<String>) -> Self {
        Self {
            link,
            video,
            initial,
            portal,
            heartbeat: HEARTBEAT,
            retry_min: RETRY_MIN,
        }
    }

    /// Shorter timings for tests.
    #[cfg(test)]
    pub fn with_timing(mut self, heartbeat: Duration, retry_min: Duration) -> Self {
        self.heartbeat = heartbeat;
        self.retry_min = retry_min;
        self
    }

    async fn serve(self, engine: EngineHandle, shutdown: Shutdown) -> Result<()> {
        let Link {
            mut commands,
            events,
            stop,
        } = self.link;
        if self.initial != LightingTarget::Off {
            // A failure lands in the snapshot and is retried below.
            let _ = engine.set_target(self.initial).await;
        }
        let mut snapshots = engine.snapshot();
        let mut sent: Option<WorkerStatus> = None;
        let mut beat = Instant::now() + self.heartbeat;
        let mut retry = Retry::new(self.retry_min);
        loop {
            let snapshot = snapshots.borrow_and_update().clone();
            if let SessionState::Faulted(phase) = snapshot.session {
                let cause = snapshot.failure.as_ref().map_or_else(
                    || Error::Faulted(phase).to_string(),
                    |failure| format!("{}: {}", Error::Faulted(phase), chain(failure)),
                );
                events.send(&Event::Faulted {
                    cause: cause.clone(),
                });
                stop.request(Stop::Faulted(cause));
                return Ok(());
            }
            let status = status(&snapshot, self.portal.as_ref());
            if sent.as_ref() != Some(&status) {
                events.send(&Event::Status(status.clone()));
                sent = Some(status);
                beat = Instant::now() + self.heartbeat;
            }
            retry.observe(&snapshot, Instant::now());
            let retry_at = retry.at;
            tokio::select! {
                () = shutdown.requested() => return Ok(()),
                command = commands.next() => match command {
                    // The link closed; the stop reason is already recorded.
                    None => return Ok(()),
                    Some(Command::Video { video }) => self.video.set(video),
                    Some(Command::ResetVideo {}) => self.video.reset(),
                    Some(Command::Lighting { target }) => {
                        retry.cancel();
                        let _ = engine.set_target(target).await;
                    }
                },
                changed = snapshots.changed() => if changed.is_err() { return Ok(()) },
                () = sleep_until(beat) => {
                    if let Some(status) = &sent {
                        events.send(&Event::Status(status.clone()));
                    }
                    beat = Instant::now() + self.heartbeat;
                }
                () = wait(retry_at) => {
                    retry.cancel();
                    let _ = engine.set_target(snapshot.target).await;
                }
            }
        }
    }
}

impl<V: VideoFeed> Surface for EngineLink<V> {
    fn run(
        self: Box<Self>,
        engine: EngineHandle,
        shutdown: Shutdown,
    ) -> BoxFuture<'static, Result<()>> {
        Box::pin(self.serve(engine, shutdown))
    }
}

async fn wait(at: Option<Instant>) {
    match at {
        Some(at) => sleep_until(at).await,
        None => std::future::pending().await,
    }
}

fn status(snapshot: &Snapshot, portal: Option<&String>) -> WorkerStatus {
    WorkerStatus {
        capturing: snapshot.session == SessionState::Running,
        target: Some(snapshot.target),
        portal: portal.cloned(),
        failure: snapshot
            .failure
            .as_ref()
            .or(snapshot.surface_failures.last())
            .map(|failure| chain(failure)),
        encoding: None,
    }
}

/// When to re-apply the target after a session failure.
struct Retry {
    min: Duration,
    delay: Duration,
    at: Option<Instant>,
    running_since: Option<Instant>,
}

impl Retry {
    fn new(min: Duration) -> Self {
        Self {
            min,
            delay: min,
            at: None,
            running_since: None,
        }
    }

    fn observe(&mut self, snapshot: &Snapshot, now: Instant) {
        if snapshot.session == SessionState::Running {
            self.running_since.get_or_insert(now);
        }
        if !wants_retry(snapshot) {
            self.at = None;
            return;
        }
        if self.at.is_some() {
            return;
        }
        if self
            .running_since
            .take()
            .is_some_and(|since| now.duration_since(since) >= HEALTHY)
        {
            self.delay = self.min;
        }
        self.at = Some(now + self.delay);
        self.delay = (self.delay * 2).min(RETRY_MAX);
    }

    fn cancel(&mut self) {
        self.at = None;
    }
}

/// A non-Off target whose session ended with a failure that a retry can fix.
/// Invalid or missing settings wait for the user instead.
fn wants_retry(snapshot: &Snapshot) -> bool {
    snapshot.target != LightingTarget::Off
        && snapshot.session == SessionState::Stopped
        && snapshot
            .failure
            .as_ref()
            .is_some_and(|failure| !matches!(failure, Error::Invalid(_) | Error::Decode { .. }))
}
