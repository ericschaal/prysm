use super::*;
use crate::screen::{Display, View};
use futures::StreamExt;
use prysm_contract::{
    FailureKind,
    identity::{FrameStamp, SettingsRevision},
    observation::Update,
};
use prysm_core::{ColorStrip, LinearColor};
use std::{
    io,
    num::{NonZeroU64, NonZeroUsize},
    time::Duration,
};
use tokio::task::JoinHandle;

const INSTANCE: InstanceId = InstanceId(NonZeroU64::MIN);

#[test]
fn typed_diagnostics_preserve_host_state_and_classify_failures_without_parsing_text() {
    use crate::diagnostics::Severity;
    use prysm_contract::{
        Failure,
        application::{
            Accepted, ApplicationEvent, ApplicationSnapshot, LightingTarget, OperationStatus,
            SessionState,
        },
        calibration::CalibrationStatus,
        capture::{CaptureCapabilities, CaptureState},
        identity::OperationId,
        observation::{Cursor, Versioned},
    };
    let terminal = TerminalOutput::new(INSTANCE);
    let diagnostics = &terminal.diagnostics;
    let cursor = Cursor {
        instance: INSTANCE,
        sequence: 1,
    };
    let id = OperationId {
        instance: INSTANCE,
        request: NonZeroU64::MIN,
    };
    diagnostics.host(HostDiagnostic::Ready(INSTANCE));
    diagnostics.host(HostDiagnostic::Accepted(&Accepted { operation: id }));
    assert_eq!(
        diagnostics.0.info().messages.back().unwrap().severity,
        Severity::Info
    );
    diagnostics.host(HostDiagnostic::Stopping);

    let snapshot = Update::Snapshot(Versioned {
        cursor,
        state: ApplicationSnapshot {
            instance: INSTANCE,
            settings: (),
            saved_revision: SettingsRevision(1),
            configured_revision: Some(SettingsRevision(2)),
            applied_revision: Some(SettingsRevision(1)),
            desired: LightingTarget::Processed,
            session: SessionState::Running(session()),
            capture: CaptureCapabilities {
                supports_patterns: false,
            },
            output: OutputCapabilities { native_mode: false },
            observed_capture: Some(CaptureState::Waiting {
                epoch: epoch(1),
                reason: "camera\nunavailable".into(),
            }),
            observed_output: None,
            calibration: CalibrationStatus::Idle,
        },
    });
    assert!(diagnostics.application(&snapshot));
    let info = diagnostics.0.info();
    assert_eq!(info.host.severity, Severity::Warning);
    assert!(info.host.text.starts_with("stopping"));
    assert!(info.application.contains("running"));
    assert_eq!(info.capture, "Capture: waiting - camera unavailable");
    assert!(info.revisions.ends_with("2 / 1"));

    let failure = Failure::new(FailureKind::Adapter, "ready\x1b[2J");
    assert!(diagnostics.application::<()>(&Update::Event {
        cursor,
        event: ApplicationEvent::OperationChanged {
            id,
            status: OperationStatus::Failed(failure.clone())
        },
    }));
    let info = diagnostics.0.info();
    let message = info.messages.back().unwrap();
    assert_eq!(message.severity, Severity::Error);
    assert_eq!(message.text, "Operation 1: ready [2J");
    assert_eq!(info.host.severity, Severity::Warning);

    diagnostics.host(HostDiagnostic::Failure(&failure));
    let info = diagnostics.0.info();
    assert_eq!(info.host.severity, Severity::Error);
    assert_eq!(info.messages.back().unwrap().severity, Severity::Error);
    diagnostics.host(HostDiagnostic::CleanupOverdue {
        pending: &["capture", "output"],
    });
    let info = diagnostics.0.info();
    assert_eq!(info.host.text, "cleanup unconfirmed");
    assert_eq!(
        info.messages.back().unwrap().text,
        "Cleanup unconfirmed: capture, output"
    );

    drop(terminal.owner);
    assert!(!diagnostics.host(HostDiagnostic::Ready(INSTANCE)));
    assert!(!diagnostics.application(&snapshot));
    assert_eq!(diagnostics.0.info().host.text, "cleanup unconfirmed");
}

fn session() -> SessionId {
    SessionId {
        instance: INSTANCE,
        sequence: NonZeroU64::MIN,
    }
}
fn epoch(generation: u64) -> StreamEpoch {
    StreamEpoch {
        session: session(),
        generation,
    }
}
fn settings() -> TerminalSettings {
    TerminalSettings { leds: [4, 2, 4, 2] }
}
fn red() -> EdgeColors {
    EdgeColors {
        top: ColorStrip::fill(LinearColor::new(1.0, 0.0, 0.0), NonZeroUsize::MIN),
        ..Default::default()
    }
}
fn frame(generation: u64, sequence: u64) -> ColorFrame {
    ColorFrame {
        stamp: FrameStamp {
            epoch: epoch(generation),
            sequence,
        },
        revision: SettingsRevision(4),
        colors: red(),
    }
}
async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(3), future)
        .await
        .expect("terminal made no progress")
}

struct Paint {
    mode: &'static str,
    colors: EdgeColors,
    stamp: Option<FrameStamp>,
    messages: Vec<String>,
    reply: std::sync::mpsc::SyncSender<io::Result<()>>,
}
impl Paint {
    fn ok(self) {
        self.reply.send(Ok(())).unwrap();
    }
    fn fail(self) {
        self.reply
            .send(Err(io::Error::other("draw failed")))
            .unwrap();
    }
}
struct GatedDisplay {
    paints: mpsc::UnboundedSender<Paint>,
    restore: mpsc::UnboundedSender<std::sync::mpsc::SyncSender<io::Result<()>>>,
}
impl Display for GatedDisplay {
    fn draw(&mut self, view: &View) -> io::Result<()> {
        let (reply, result) = std::sync::mpsc::sync_channel(1);
        self.paints
            .send(Paint {
                mode: view.mode.label(),
                colors: view.colors.clone(),
                stamp: view.frame.as_ref().map(|f| f.stamp),
                messages: view
                    .info
                    .messages
                    .iter()
                    .map(|message| message.text.clone())
                    .collect(),
                reply,
            })
            .unwrap();
        result.recv().unwrap()
    }
    fn restore(&mut self) -> io::Result<()> {
        let (reply, result) = std::sync::mpsc::sync_channel(1);
        self.restore.send(reply).unwrap();
        result.recv().unwrap()
    }
}
struct Running {
    control: TerminalControl,
    colors: TerminalColors,
    diagnostics: TerminalDiagnostics,
    shutdown: TerminalShutdown,
    paints: mpsc::UnboundedReceiver<Paint>,
    restores: mpsc::UnboundedReceiver<std::sync::mpsc::SyncSender<io::Result<()>>>,
    task: JoinHandle<Result<OwnerStopped, ControlError>>,
}
impl Running {
    async fn new() -> Self {
        let terminal = TerminalOutput::new(INSTANCE);
        let (paints, receiver) = mpsc::unbounded_channel();
        let (restore, restores) = mpsc::unbounded_channel();
        let task = tokio::spawn(worker::run(
            terminal.owner,
            std::future::pending(),
            move || Ok(GatedDisplay { paints, restore }),
        ));
        let mut running = Self {
            control: terminal.control,
            colors: terminal.colors,
            diagnostics: terminal.diagnostics,
            shutdown: terminal.shutdown,
            paints: receiver,
            restores,
            task,
        };
        running.paint("idle").await.ok();
        let control = running.control.clone();
        let configure = tokio::spawn(async move { control.configure(settings()).await });
        running.paint("configured").await.ok();
        within(configure).await.unwrap().unwrap();
        running
    }
    async fn paint(&mut self, mode: &str) -> Paint {
        self.paint_matching(|paint| paint.mode == mode).await
    }
    async fn paint_matching(&mut self, matches: impl Fn(&Paint) -> bool) -> Paint {
        within(async {
            loop {
                let paint = self.paints.recv().await.unwrap();
                if matches(&paint) {
                    return paint;
                }
                paint.ok(); // A periodic redraw may precede the queued command.
            }
        })
        .await
    }
    async fn hold(&mut self, generation: u64) {
        let control = self.control.clone();
        let mut request =
            tokio::spawn(
                async move { control.hold(epoch(generation), EdgeColors::default()).await },
            );
        within(async {
            loop {
                tokio::select! {
                    biased;
                    result = &mut request => { result.unwrap().unwrap(); break; }
                    Some(paint) = self.paints.recv() => paint.ok(),
                }
            }
        })
        .await;
    }
    async fn close(mut self) {
        self.shutdown.request();
        // A scheduled redraw may already be owned before the fence.
        let restore = within(async {
            loop {
                tokio::select! {
                    Some(paint) = self.paints.recv() => paint.ok(),
                    restore = self.restores.recv() => break restore.unwrap(),
                }
            }
        })
        .await;
        assert!(!self.task.is_finished());
        restore.send(Ok(())).unwrap();
        within(self.task).await.unwrap().unwrap();
        assert!(!self.diagnostics.host(HostDiagnostic::Stopping));
    }
}

#[tokio::test]
async fn latest_frames_obey_admission_and_diagnostics_cannot_revive_finished_colors() {
    let mut terminal = Running::new().await;
    terminal.hold(0).await;
    for sequence in 1..=100 {
        terminal.colors.offer(frame(2, sequence)).unwrap();
    }
    let control = terminal.control.clone();
    let admit = tokio::spawn(async move { control.admit(epoch(1)).await });
    let paint = terminal.paint("streaming").await;
    assert_eq!(paint.colors, EdgeColors::default());
    assert!(paint.stamp.is_none());
    paint.ok();
    within(admit).await.unwrap().unwrap();
    assert_eq!(
        terminal
            .control
            .observe()
            .await
            .unwrap()
            .initial
            .state
            .admitted,
        Some(epoch(1))
    );
    let control = terminal.control.clone();
    let admit = tokio::spawn(async move { control.admit(epoch(2)).await });
    let paint = terminal
        .paint_matching(|paint| paint.stamp == Some(frame(2, 100).stamp))
        .await;
    assert_eq!(paint.colors, red());
    paint.ok();
    within(admit).await.unwrap().unwrap();
    terminal.hold(2).await;
    assert_eq!(
        terminal.colors.offer(frame(2, 101)).unwrap(),
        OfferResult::DiscardedStale
    );
    assert!(terminal.control.admit(epoch(2)).await.is_err());
    let control = terminal.control.clone();
    let finish = tokio::spawn(async move { control.finish(session(), IdleOutputMode::Off).await });
    terminal.paint("off").await.ok();
    within(finish).await.unwrap().unwrap();
    for sequence in 0..20 {
        let failure = prysm_contract::Failure::new(
            FailureKind::Adapter,
            format!("update {sequence}\x1b[2J\n"),
        );
        terminal
            .diagnostics
            .host(HostDiagnostic::CommandRejected(&failure));
    }
    let paint = terminal
        .paint_matching(|paint| paint.mode == "off" && paint.messages.len() == 6)
        .await;
    assert_eq!(paint.colors, EdgeColors::default());
    assert!(paint.stamp.is_none());
    assert_eq!(paint.messages.len(), 6);
    assert!(
        paint
            .messages
            .iter()
            .all(|m| !m.chars().any(char::is_control))
    );
    paint.ok();
    terminal.close().await;
}

#[tokio::test]
async fn shutdown_fences_immediately_but_joins_only_after_draw_and_restore() {
    for (draw_fails, restore_fails) in [(false, false), (true, false), (true, true)] {
        let mut terminal = Running::new().await;
        let control = terminal.control.clone();
        let request = tokio::spawn(async move { control.hold(epoch(0), red()).await });
        let paint = terminal.paint("held").await;
        assert!(!request.is_finished());
        request.abort(); // Accepted work belongs to the output owner.
        within(request).await.unwrap_err();
        terminal.shutdown.request();
        terminal.shutdown.request();
        assert!(terminal.colors.offer(frame(1, 1)).is_err());
        assert!(within(terminal.control.admit(epoch(1))).await.is_err());
        assert!(within(terminal.control.observe()).await.is_ok());
        assert!(!terminal.task.is_finished());
        if draw_fails {
            paint.fail();
        } else {
            paint.ok();
        }
        let restore = within(terminal.restores.recv()).await.unwrap();
        assert!(!terminal.task.is_finished());
        restore
            .send(if restore_fails {
                Err(io::Error::other("restore failed"))
            } else {
                Ok(())
            })
            .unwrap();
        let result = within(terminal.task).await.unwrap();
        if draw_fails {
            assert!(
                result
                    .as_ref()
                    .unwrap_err()
                    .to_string()
                    .contains("draw failed")
            );
        }
        if restore_fails {
            assert_eq!(result.unwrap_err().kind(), FailureKind::RecoveryRequired);
        } else {
            assert_eq!(result.is_err(), draw_fails);
        }
        assert!(terminal.control.observe().await.is_err());
    }
}

#[tokio::test]
async fn unsupported_native_invalid_config_and_observation_gaps_are_explicit() {
    let mut terminal = Running::new().await;
    assert!(!terminal.control.capabilities().native_mode);
    assert!(
        terminal
            .control
            .configure(TerminalSettings { leds: [0; 4] })
            .await
            .is_err()
    );
    assert!(
        terminal
            .control
            .finish(session(), IdleOutputMode::Native)
            .await
            .is_err()
    );
    let mut observation = terminal.control.observe().await.unwrap();
    for _ in 0..20 {
        terminal.hold(0).await;
    }
    assert!(matches!(
        within(observation.updates.next()).await.unwrap().unwrap(),
        Update::ResyncRequired
    ));
    terminal.close().await;
}

#[tokio::test]
async fn queued_controls_survive_worker_registration_and_preclosed_owner_never_opens_screen() {
    let terminal = TerminalOutput::new(INSTANCE);
    let control = terminal.control.clone();
    let configure = tokio::spawn(async move { control.configure(settings()).await });
    tokio::task::yield_now().await;
    let (paints, mut received) = mpsc::unbounded_channel();
    let (restore, mut restores) = mpsc::unbounded_channel();
    let owner = tokio::spawn(worker::run(
        terminal.owner,
        std::future::pending(),
        move || Ok(GatedDisplay { paints, restore }),
    ));
    within(received.recv()).await.unwrap().ok();
    within(configure).await.unwrap().unwrap();
    terminal.shutdown.request();
    let restore = within(async {
        loop {
            tokio::select! {
                Some(paint) = received.recv() => paint.ok(),
                Some(restore) = restores.recv() => break restore,
            }
        }
    })
    .await;
    restore.send(Ok(())).unwrap();
    within(owner).await.unwrap().unwrap();

    let terminal = TerminalOutput::new(INSTANCE);
    terminal.shutdown.request();
    within(worker::run::<GatedDisplay>(
        terminal.owner,
        std::future::pending(),
        || panic!("closed output must not acquire a terminal"),
    ))
    .await
    .unwrap();
}
