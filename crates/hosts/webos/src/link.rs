//! The engine's side of the supervisor link: commands in, status out.
use futures::future::BoxFuture;
use prysm_capture_webos::VideoSignal;
use prysm_ports::{Error, LightingTarget, Result, SessionPhase, SessionState, Snapshot, chain};
use prysm_runtime::{EngineHandle, Shutdown, Surface};
use prysm_webos_link::worker::{Link, Stop};
use prysm_webos_link::{
    Backoff, Command, Event, FullStatus, HEARTBEAT, WorkerFailure, WorkerStatus,
};
use std::time::Duration;
use tokio::time::{Instant, sleep_until};

/// Where people open the portal right now (its LAN address can change).
pub type PortalUrl = Box<dyn Fn() -> Option<String> + Send>;

/// `Surface<EngineHandle>`: applies the initial target, then turns link
/// commands into video signals and targets, reports every snapshot change
/// (and a heartbeat) as status (the last one before a fault too),
/// re-applies a non-Off target after a session failure with [`Backoff`],
/// stops the worker on a faulted session, and fails it (for the supervisor
/// to start a new one) on a capture failure no source in this process can
/// recover from ([`needs_restart`]). The portal URL is looked up again
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
            // One clock read per round, so a retry and a heartbeat set in the
            // same round fall due together.
            let now = Instant::now();
            let snapshot = snapshots.borrow_and_update().clone();
            let status = status(&snapshot, portal.as_deref());
            if sent.as_ref() != Some(&status) {
                events.send(&Event::Status(status.clone()));
                sent = Some(status);
                beat = now + self.heartbeat;
            }
            if let SessionState::Faulted(phase) = snapshot.session {
                let cause = faulted(phase, snapshot.failure.as_ref());
                events.send(&Event::Faulted {
                    cause: cause.clone(),
                });
                stop.request(Stop::Faulted(cause));
                return Ok(());
            }
            if let Some(cause) = needs_restart(&snapshot) {
                events.send(&Event::Failed {
                    cause: cause.clone(),
                });
                stop.request(Stop::Failed(cause));
                return Ok(());
            }
            retry.observe(&snapshot, now);
            let retry_at = retry.at;
            // In this order when several are ready: a retry due with the
            // heartbeat is applied before the heartbeat's status is sent.
            tokio::select! {
                biased;
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
                () = wait(retry_at) => {
                    retry.cancel();
                    apply(&engine, snapshot.target).await;
                }
                // The heartbeat: the status again, with the current portal URL.
                () = sleep_until(beat) => {
                    portal = (self.portal_url)();
                    sent = None;
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
    WorkerStatus::Full(FullStatus {
        // Running and frames arriving: false while waiting for video.
        capturing: snapshot.session == SessionState::Running
            && snapshot.input.is_some_and(|input| input.receiving),
        fps: snapshot.input.map(|input| whole_fps(input.fps)),
        target: snapshot.target,
        portal: portal.map(str::to_owned),
        failure: failure.map(|failure| WorkerFailure::other(chain(failure))),
    })
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

/// Why the worker must restart to capture again, when its session ended with
/// a capture failure no source in this process can recover from: the
/// worker then fails (exit 1), and the supervisor starts a new one after its
/// backoff instead of this worker retrying in place.
fn needs_restart(snapshot: &Snapshot) -> Option<String> {
    let failure = snapshot.failure.as_ref()?;
    (snapshot.session == SessionState::Stopped && prysm_capture_webos::needs_new_process(failure))
        .then(|| chain(failure))
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

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_ports::ValidationError;

    const FIRST: Duration = Duration::from_millis(100);

    /// What a snapshot says, as far as retrying goes.
    #[derive(Debug, Clone, Copy)]
    enum Seen {
        /// Lighting off after a failure: nothing to retry.
        Off,
        Running,
        /// Stopped with a failure a retry can fix.
        Failed,
        /// Stopped because the settings are invalid: the user must act.
        Invalid,
    }

    #[derive(Debug, Clone, Copy)]
    enum Event {
        Observe(Seen),
        /// A command, or the retry itself, applied a target.
        Cancel,
    }

    fn snapshot(seen: Seen) -> Snapshot {
        let stopped = |target, failure| Snapshot {
            target,
            failure: Some(failure),
            ..Snapshot::default()
        };
        match seen {
            Seen::Off => stopped(LightingTarget::Off, Error::other("source failed")),
            Seen::Running => Snapshot {
                target: LightingTarget::Processed,
                session: SessionState::Running,
                ..Snapshot::default()
            },
            Seen::Failed => stopped(LightingTarget::Processed, Error::other("source failed")),
            Seen::Invalid => stopped(
                LightingTarget::Processed,
                Error::Invalid(ValidationError::new("output", "missing")),
            ),
        }
    }

    /// Only a stopped session whose capture cannot recover in this process
    /// restarts the worker; a running one, or any other failure, does not.
    #[test]
    fn only_an_unrecoverable_capture_failure_restarts_the_worker() {
        use prysm_capture_webos_sys as sys;
        let unrecoverable = || {
            Error::adapter(
                "cannot open webOS video capture",
                sys::Error::Create(Box::new(sys::Error::Exception("bad_alloc".into()))),
            )
        };
        let stopped = Snapshot {
            target: LightingTarget::Processed,
            failure: Some(unrecoverable()),
            ..Snapshot::default()
        };
        assert_eq!(
            needs_restart(&stopped).as_deref(),
            Some(
                "cannot open webOS video capture: vtCapture_create failed: vtcapture exception: bad_alloc"
            )
        );
        let running = Snapshot {
            session: SessionState::Running,
            ..stopped
        };
        assert_eq!(needs_restart(&running), None);
        for seen in [Seen::Off, Seen::Running, Seen::Failed, Seen::Invalid] {
            assert_eq!(needs_restart(&snapshot(seen)), None, "{seen:?}");
        }
    }

    fn apply(retry: &mut Retry, event: Event, now: Instant) {
        match event {
            Event::Observe(seen) => retry.observe(&snapshot(seen), now),
            Event::Cancel => retry.cancel(),
        }
    }

    /// Every reachable state meets every event: when the retry is due and
    /// since when a session has run, afterwards. (A scheduled retry with a
    /// running session cannot happen: running clears the schedule.)
    #[test]
    fn every_state_meets_every_event() {
        let t0 = Instant::now();
        let now = t0 + Duration::from_secs(10);
        let due = t0 + Duration::from_secs(5);
        let idle = (None, None);
        let scheduled = (Some(due), None);
        let ran = (None, Some(t0));
        let first = Some(now + FIRST);
        #[rustfmt::skip]
        let cases = [
            (idle, Event::Observe(Seen::Off), (None, None)),
            (idle, Event::Observe(Seen::Running), (None, Some(now))),
            (idle, Event::Observe(Seen::Failed), (first, None)),
            (idle, Event::Observe(Seen::Invalid), (None, None)),
            (idle, Event::Cancel, (None, None)),
            // Lighting turned off cancels a scheduled retry.
            (scheduled, Event::Observe(Seen::Off), (None, None)),
            (scheduled, Event::Observe(Seen::Running), (None, Some(now))),
            // The same failure seen again keeps its time.
            (scheduled, Event::Observe(Seen::Failed), (Some(due), None)),
            (scheduled, Event::Observe(Seen::Invalid), (None, None)),
            (scheduled, Event::Cancel, (None, None)),
            (ran, Event::Observe(Seen::Off), (None, Some(t0))),
            (ran, Event::Observe(Seen::Running), (None, Some(t0))),
            (ran, Event::Observe(Seen::Failed), (first, None)),
            (ran, Event::Observe(Seen::Invalid), (None, Some(t0))),
            (ran, Event::Cancel, (None, Some(t0))),
        ];
        for ((at, running_since), event, expected) in cases {
            let mut retry = Retry {
                backoff: Backoff::starting_at(FIRST),
                at,
                running_since,
            };
            apply(&mut retry, event, now);
            assert_eq!(
                (retry.at, retry.running_since),
                expected,
                "{at:?} {running_since:?} meets {event:?}"
            );
        }
    }

    /// Each failure in a row waits twice as long as the one before, until a
    /// session has run for [`Backoff::HEALTHY`].
    #[test]
    fn the_delay_doubles_until_a_session_runs_healthy() {
        let t0 = Instant::now();
        let mut retry = Retry::new(Backoff::starting_at(FIRST));
        let fail_at = |retry: &mut Retry, now: Instant| {
            apply(retry, Event::Observe(Seen::Failed), now);
            let due = retry.at.expect("a retry is scheduled");
            apply(retry, Event::Cancel, due);
            due - now
        };
        assert_eq!(fail_at(&mut retry, t0), FIRST);
        assert_eq!(fail_at(&mut retry, t0), FIRST * 2);
        // A session too short to count as healthy keeps doubling.
        let started = t0 + Duration::from_secs(1);
        apply(&mut retry, Event::Observe(Seen::Running), started);
        let short = started + Backoff::HEALTHY - Duration::from_millis(1);
        assert_eq!(fail_at(&mut retry, short), FIRST * 4);
        apply(&mut retry, Event::Observe(Seen::Running), short);
        let healthy = short + Backoff::HEALTHY;
        assert_eq!(fail_at(&mut retry, healthy), FIRST);
    }
}
