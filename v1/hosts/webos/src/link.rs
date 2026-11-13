//! The engine's side of the supervisor link: commands in, status out.
use futures::future::BoxFuture;
use prysm_capture_webos::VideoSignal;
use prysm_ports::{Error, LightingTarget, Result, SessionPhase, SessionState, Snapshot, chain};
use prysm_runtime::{EngineHandle, Shutdown, Surface};
use prysm_webos_link::worker::{Link, Stop};
use prysm_webos_link::{Backoff, Command, Event, HEARTBEAT, WorkerFailure, WorkerStatus};
use std::time::Duration;
use tokio::time::{Instant, sleep_until};

/// Where people open the portal right now (its LAN address can change).
pub type PortalUrl = Box<dyn Fn() -> Option<String> + Send>;

/// `Surface<EngineHandle>`: applies the initial target, then turns link
/// commands into video signals and targets, reports every snapshot change
/// (and a heartbeat) as status (the last one before a fault too),
/// re-applies a non-Off target after a session failure with [`Backoff`], and
/// stops the worker on a faulted session. The portal URL is looked up again
/// at every heartbeat, so a new LAN address is reported within one.
pub struct EngineLink {
    link: Link,
    video: VideoSignal,
    initial: LightingTarget,
    portal_url: PortalUrl,
    heartbeat: Duration,
    backoff: Backoff,
}

impl EngineLink {
    pub fn new(
        link: Link,
        video: VideoSignal,
        initial: LightingTarget,
        portal_url: PortalUrl,
    ) -> Self {
        Self {
            link,
            video,
            initial,
            portal_url,
            heartbeat: HEARTBEAT,
            backoff: Backoff::default(),
        }
    }

    /// Shorter timings for tests.
    #[cfg(test)]
    pub fn with_timing(mut self, heartbeat: Duration, first_retry: Duration) -> Self {
        self.heartbeat = heartbeat;
        self.backoff = Backoff::starting_at(first_retry);
        self
    }

    async fn serve(self, engine: EngineHandle, shutdown: Shutdown) -> Result<()> {
        let Link {
            mut commands,
            events,
            stop,
        } = self.link;
        if self.initial != LightingTarget::Off {
            apply(&engine, self.initial).await;
        }
        let mut snapshots = engine.snapshot();
        let mut portal = (self.portal_url)();
        let mut sent: Option<WorkerStatus> = None;
        let mut beat = Instant::now() + self.heartbeat;
        let mut retry = Retry::new(self.backoff);
        loop {
            let snapshot = snapshots.borrow_and_update().clone();
            let status = status(&snapshot, portal.as_deref());
            if sent.as_ref() != Some(&status) {
                events.send(&Event::Status(status.clone()));
                sent = Some(status);
                beat = Instant::now() + self.heartbeat;
            }
            if let SessionState::Faulted(phase) = snapshot.session {
                let cause = faulted(phase, snapshot.failure.as_ref());
                events.send(&Event::Faulted {
                    cause: cause.clone(),
                });
                stop.request(Stop::Faulted(cause));
                return Ok(());
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
                        apply(&engine, target).await;
                    }
                },
                changed = snapshots.changed() => if changed.is_err() { return Ok(()) },
                // The heartbeat: the status again, with the current portal URL.
                () = sleep_until(beat) => {
                    portal = (self.portal_url)();
                    sent = None;
                }
                () = wait(retry_at) => {
                    retry.cancel();
                    apply(&engine, snapshot.target).await;
                }
            }
        }
    }
}

impl Surface for EngineLink {
    fn run(
        self: Box<Self>,
        engine: EngineHandle,
        shutdown: Shutdown,
    ) -> BoxFuture<'static, Result<()>> {
        Box::pin(self.serve(engine, shutdown))
    }
}

/// Ask for `target`. A failure is not returned: it lands in the snapshot,
/// which reports it and retries it ([`Retry`]).
async fn apply(engine: &EngineHandle, target: LightingTarget) {
    let _ = engine.set_target(target).await;
}

/// Why the session faulted, with the failure's causes when there is one.
fn faulted(phase: SessionPhase, failure: Option<&Error>) -> String {
    let fault = Error::Faulted(phase);
    match failure {
        Some(failure) => format!("{fault}: {}", chain(failure)),
        None => fault.to_string(),
    }
}

async fn wait(at: Option<Instant>) {
    match at {
        Some(at) => sleep_until(at).await,
        None => std::future::pending().await,
    }
}

fn status(snapshot: &Snapshot, portal: Option<&str>) -> WorkerStatus {
    let failure = snapshot
        .failure
        .as_ref()
        .or(snapshot.surface_failures.last());
    WorkerStatus {
        // Running and frames arriving: false while waiting for video.
        capturing: snapshot.session == SessionState::Running
            && snapshot.input.is_some_and(|input| input.receiving),
        fps: snapshot.input.map(|input| whole_fps(input.fps)),
        target: Some(snapshot.target),
        portal: portal.map(str::to_owned),
        failure: failure.map(|failure| WorkerFailure::other(chain(failure))),
        encoding: None,
        paused: false,
    }
}

/// A measured frame rate, rounded to whole frames per second.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "rounded and clamped to the u16 range first"
)]
fn whole_fps(fps: f32) -> u16 {
    fps.round().clamp(0.0, f32::from(u16::MAX)) as u16
}

/// When to re-apply the target after a session failure.
struct Retry {
    backoff: Backoff,
    at: Option<Instant>,
    running_since: Option<Instant>,
}

impl Retry {
    fn new(backoff: Backoff) -> Self {
        Self {
            backoff,
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
            .is_some_and(|since| now.duration_since(since) >= Backoff::HEALTHY)
        {
            self.backoff.reset();
        }
        self.at = Some(now + self.backoff.delay());
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
