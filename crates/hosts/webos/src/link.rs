//! The engine's side of the supervisor link: commands in, status out.
use futures::future::BoxFuture;
use prysm_capture_webos::VideoSignal;
use prysm_ports::{Error, LightingMode, LightingTarget, Result, SessionState, Snapshot};
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
    portal_url: PortalUrl,
    heartbeat: Duration,
}

impl EngineLink {
    pub fn new(link: Link, video: VideoSignal, portal_url: PortalUrl) -> Self {
        Self {
            link,
            video,
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
        let mut snapshots = engine.snapshot();
        let mut portal = (self.portal_url)();
        let mut sent: Option<WorkerStatus> = None;
        let mut beat = Instant::now() + self.heartbeat;
        let mut applied = 0;
        let mut requested = None;
        let mut initialized = false;
        let mut video_initialized = false;
        let mut video_seen = false;
        loop {
            let snapshot = snapshots.borrow_and_update().clone();
            let status = status(&snapshot, portal.as_deref(), applied);
            if sent.as_ref() != Some(&status) {
                events.send(&Event::Status(status.clone()));
                sent = Some(status);
                beat = Instant::now() + self.heartbeat;
            }
            if !initialized
                && video_seen
                && operation_initialized(
                    &snapshot,
                    requested.unwrap_or(snapshot.target),
                    video_initialized,
                )
            {
                events.send(&Event::Initialized {});
                initialized = true;
            }
            tokio::select! {
                biased;
                () = shutdown.requested() => return Ok(()),
                () = self.video.initialized(), if !initialized && !video_initialized => {
                    video_initialized = true;
                }
                command = commands.next() => match command {
                    // The link closed; the stop reason is already recorded.
                    None => return Ok(()),
                    Some(Command::Video { video }) => {
                        self.video.set(video);
                        video_seen = true;
                    }
                    Some(Command::ResetVideo {}) => self.video.reset(),
                    Some(Command::ObserveVideo { available }) => self.video.observe(available),
                    Some(Command::Target { target, sequence }) => {
                        apply(&engine, target).await;
                        applied = sequence;
                        requested = Some(target);
                    }
                    Some(Command::Resume { sequence }) => {
                        let target = match snapshots.borrow().control.remembered {
                            LightingMode::ScreenSync => LightingTarget::ScreenSync,
                            LightingMode::Effects => LightingTarget::Effects,
                        };
                        apply(&engine, target).await;
                        applied = sequence;
                        requested = Some(target);
                    }
                    Some(Command::RecoverScreenSync { sequence }) => {
                        let _ = engine.resume_screen_sync().await;
                        applied = sequence;
                        // An authoritative override can fulfill the recovered
                        // worker in Effects instead of Screen sync.
                        requested = None;
                    }
                },
                changed = snapshots.changed() => if changed.is_err() { return Ok(()) },
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

fn operation_initialized(
    snapshot: &Snapshot,
    requested: LightingTarget,
    video_initialized: bool,
) -> bool {
    snapshot.control.waiting.is_none()
        && snapshot.session.reached(requested)
        && (requested != LightingTarget::ScreenSync || video_initialized)
        && failure(snapshot).is_none()
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
        session => {
            let adapter_failure = if snapshot.control.waiting.is_none() {
                session.failure()
            } else {
                None
            };
            adapter_failure
                .or(snapshot.surface_failures.last())
                .map(Failure::of)
        }
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

    #[test]
    fn stable_screen_sync_requires_initialized_video_and_the_running_session() {
        let mut snapshot = Snapshot {
            target: LightingTarget::ScreenSync,
            session: SessionState::ScreenSync,
            ..Snapshot::default()
        };
        assert!(!operation_initialized(
            &snapshot,
            LightingTarget::ScreenSync,
            false
        ));
        assert!(operation_initialized(
            &snapshot,
            LightingTarget::ScreenSync,
            true
        ));
        snapshot.session = SessionState::Stopped { failure: None };
        assert!(!operation_initialized(
            &snapshot,
            LightingTarget::ScreenSync,
            true
        ));
        assert!(operation_initialized(&snapshot, LightingTarget::Off, false));
        snapshot.session = SessionState::Effects {
            failure: Some(Error::other("offline")),
        };
        assert!(!operation_initialized(
            &snapshot,
            LightingTarget::Effects,
            false
        ));
    }

    #[test]
    fn a_waiting_adapter_keeps_its_cause_without_asking_to_restart_or_retry() {
        let mut snapshot = Snapshot {
            target: LightingTarget::ScreenSync,
            session: SessionState::Stopped {
                failure: Some(Error::other("no frame")),
            },
            ..Snapshot::default()
        };
        snapshot.control.waiting = Some(prysm_ports::WaitingFor::Capture);
        assert_eq!(failure(&snapshot), None);
        assert!(!operation_initialized(
            &snapshot,
            LightingTarget::ScreenSync,
            true
        ));
        snapshot.control.waiting = None;
        assert_eq!(failure(&snapshot).unwrap().message, "no frame");
    }

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
