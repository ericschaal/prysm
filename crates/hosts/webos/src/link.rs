//! The engine's side of the supervisor link: commands in, status out.
use futures::future::BoxFuture;
use prysm_capture_webos::VideoSignal;
use prysm_ports::{Error, LightingTarget, Result, SessionState, Snapshot};
use prysm_runtime::{EngineHandle, Shutdown, Surface};
use prysm_webos_link::worker::Link;
use prysm_webos_link::{Command, Event, Failure, FullStatus, HEARTBEAT, WorkerStatus};
use std::time::Duration;
use tokio::time::{Instant, sleep_until};

/// Where people open the portal right now (its LAN address can change).
pub type PortalUrl = Box<dyn Fn() -> Option<String> + Send>;

/// `Surface<EngineHandle>`: applies the initial target, then turns link
/// commands into video signals and targets and reports every snapshot
/// change (and a heartbeat) as status. A failed or faulted session is in
/// the status with its escalation ([`failure`]) and stays as it is until
/// the next command: the worker decides nothing about it. The portal URL is
/// looked up again at every heartbeat, so a new LAN address is reported
/// within one.
pub struct EngineLink {
    link: Link,
    video: VideoSignal,
    initial: LightingTarget,
    portal_url: PortalUrl,
    heartbeat: Duration,
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
        }
    }

    /// A shorter heartbeat for tests.
    #[cfg(test)]
    pub fn with_heartbeat(mut self, heartbeat: Duration) -> Self {
        self.heartbeat = heartbeat;
        self
    }

    async fn serve(self, engine: EngineHandle, shutdown: Shutdown) -> Result<()> {
        let Link {
            mut commands,
            events,
            ..
        } = self.link;
        if self.initial != LightingTarget::Off {
            apply(&engine, self.initial).await;
        }
        let mut snapshots = engine.snapshot();
        let mut portal = (self.portal_url)();
        let mut sent: Option<WorkerStatus> = None;
        let mut beat = Instant::now() + self.heartbeat;
        // The last target command applied: 0 until one is.
        let mut applied = 0;
        loop {
            let snapshot = snapshots.borrow_and_update().clone();
            let status = status(&snapshot, portal.as_deref(), applied);
            if sent.as_ref() != Some(&status) {
                events.send(&Event::Status(status.clone()));
                sent = Some(status);
                beat = Instant::now() + self.heartbeat;
            }
            tokio::select! {
                biased;
                () = shutdown.requested() => return Ok(()),
                command = commands.next() => match command {
                    // The link closed; the stop reason is already recorded.
                    None => return Ok(()),
                    Some(Command::Video { video }) => self.video.set(video),
                    Some(Command::ResetVideo {}) => self.video.reset(),
                    Some(Command::Target { target, sequence }) => {
                        apply(&engine, target).await;
                        applied = sequence;
                    }
                },
                changed = snapshots.changed() => if changed.is_err() { return Ok(()) },
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
/// which the status reports.
async fn apply(engine: &EngineHandle, target: LightingTarget) {
    let _ = engine.set_target(target).await;
}

/// The status after the target command `applied`.
fn status(snapshot: &Snapshot, portal: Option<&str>, applied: u64) -> WorkerStatus {
    WorkerStatus::Full(FullStatus {
        // Running and frames arriving: false while waiting for video.
        capturing: matches!(snapshot.session, SessionState::ScreenSync)
            && snapshot
                .input
                .is_some_and(|input| input.picture.is_arriving()),
        fps: snapshot.input.map(|input| whole_fps(input.fps)),
        target: snapshot.target,
        lights: snapshot.device.issue,
        applied,
        portal: portal.map(str::to_owned),
        failure: failure(snapshot),
    })
}

/// What the status reports as the failure: a faulted session's
/// ([`Error::Faulted`], escalated as `Restart`), else the session's, else
/// the last surface failure; each copied with its escalation.
fn failure(snapshot: &Snapshot) -> Option<Failure> {
    match &snapshot.session {
        SessionState::Faulted(phase) => Some(Failure::of(&Error::Faulted(*phase))),
        session => session
            .failure()
            .or(snapshot.surface_failures.last())
            .map(Failure::of),
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

#[cfg(test)]
mod tests {
    use super::*;
    use prysm_ports::{Escalation, Fix, SessionPhase, ValidationError};

    /// A faulted session is reported as `Restart`, before any surface
    /// failure; another session's failure, or else the last surface
    /// failure, with its own escalation; nothing when there is none.
    #[test]
    fn the_status_copies_the_failure_and_its_escalation() {
        let portal = || Error::other("portal stopped");
        let snapshot = |session| Snapshot {
            target: LightingTarget::ScreenSync,
            session,
            surface_failures: vec![portal()],
            ..Snapshot::default()
        };
        let invalid = || Error::Invalid(ValidationError::new("output", "missing"));
        let cases = [
            (
                snapshot(SessionState::Faulted(SessionPhase::SourceUnreleased)),
                Some(Failure::of(&Error::Faulted(SessionPhase::SourceUnreleased))),
            ),
            (
                snapshot(SessionState::Stopped {
                    failure: Some(invalid()),
                }),
                Some(Failure::of(&invalid())),
            ),
            (
                snapshot(SessionState::ScreenSync),
                Some(Failure::of(&portal())),
            ),
            (Snapshot::default(), None),
        ];
        for (snapshot, expected) in cases {
            assert_eq!(failure(&snapshot), expected, "{}", snapshot.session);
        }
        let escalation = |snapshot| failure(&snapshot).map(|failure| failure.escalation);
        assert_eq!(
            escalation(snapshot(SessionState::Faulted(SessionPhase::Running))),
            Some(Escalation::Restart)
        );
        assert_eq!(
            escalation(snapshot(SessionState::Stopped {
                failure: Some(invalid())
            })),
            Some(Escalation::Ask(Fix::ChangeSettings))
        );
    }

    /// The status says the lights' issue of the running session, the wait
    /// for lights that do not answer included, and none without one.
    #[test]
    fn the_status_carries_the_lights_issue() {
        use prysm_ports::{DeviceReport, OutputIssue};
        let running = |issue| Snapshot {
            target: LightingTarget::ScreenSync,
            session: SessionState::ScreenSync,
            device: DeviceReport {
                issue,
                ..DeviceReport::default()
            },
            ..Snapshot::default()
        };
        for issue in [None, Some(OutputIssue::Unreachable), Some(OutputIssue::Off)] {
            let WorkerStatus::Full(full) = status(&running(issue), None, 0) else {
                panic!("a full status");
            };
            assert_eq!(full.lights, issue);
        }
    }
}
