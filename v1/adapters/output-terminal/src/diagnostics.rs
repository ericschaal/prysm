use crate::mailbox::Mailbox;
use prysm_contract::{
    Failure,
    application::{
        Accepted, ApplicationEvent, ApplicationSnapshot, LightingTarget, OperationStatus,
        SessionState,
    },
    capture::CaptureState,
    identity::InstanceId,
    observation::Update,
};
use std::{collections::VecDeque, sync::Arc};

/// Presentation severity is derived from typed facts inside this adapter.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(super) enum Severity {
    #[default]
    Info,
    Warning,
    Error,
}

#[derive(Clone, Default)]
pub(super) struct Diagnostic {
    pub severity: Severity,
    pub text: String,
}

#[derive(Clone, Default)]
pub(super) struct Info {
    pub host: Diagnostic,
    pub application: String,
    pub capture: String,
    pub revisions: String,
    pub messages: VecDeque<Diagnostic>,
}

/// Host facts for display; wording and severity belong to the terminal adapter.
/// This input cannot control the host or change output admission.
pub enum HostDiagnostic<'a> {
    Ready(InstanceId),
    Stopping,
    Accepted(&'a Accepted),
    CommandRejected(&'a Failure),
    Failure(&'a Failure),
    CleanupOverdue { pending: &'a [&'a str] },
}

/// Optional presentation input. It cannot mutate the application or start output.
/// Methods return false once the screen owner has closed, allowing fallback logging.
#[derive(Clone)]
pub struct TerminalDiagnostics(pub(super) Arc<Mailbox>);
impl TerminalDiagnostics {
    pub fn host(&self, event: HostDiagnostic<'_>) -> bool {
        self.0.diagnostics(|info| match event {
            HostDiagnostic::Ready(instance) => {
                info.status(Severity::Info, "ready");
                info.message(Severity::Info, &format!("Ready, instance {}", instance.0));
            }
            HostDiagnostic::Stopping => info.status(
                Severity::Warning,
                "stopping; awaiting capture and output cleanup",
            ),
            HostDiagnostic::Accepted(accepted) => info.message(
                Severity::Info,
                &format!("Operation {} accepted", accepted.operation.request),
            ),
            HostDiagnostic::CommandRejected(failure) => {
                info.message(Severity::Error, &failure.to_string())
            }
            HostDiagnostic::Failure(failure) => {
                info.status(Severity::Error, "terminal cleanup after failure");
                info.message(Severity::Error, &failure.to_string());
            }
            HostDiagnostic::CleanupOverdue { pending } => {
                info.status(Severity::Error, "cleanup unconfirmed");
                info.message(
                    Severity::Error,
                    &format!("Cleanup unconfirmed: {}", pending.join(", ")),
                );
            }
        })
    }

    pub fn application<S>(
        &self,
        update: &Update<ApplicationSnapshot<S>, ApplicationEvent>,
    ) -> bool {
        self.0.diagnostics(|info| match update {
            Update::Snapshot(snapshot) => info.application(&snapshot.state),
            Update::Event {
                event: ApplicationEvent::OperationChanged { id, status },
                ..
            } => {
                let (severity, status) = match status {
                    OperationStatus::Pending => (Severity::Info, "pending".into()),
                    OperationStatus::Succeeded => (Severity::Info, "complete".into()),
                    OperationStatus::Failed(failure) => (Severity::Error, failure.to_string()),
                };
                info.message(severity, &format!("Operation {}: {status}", id.request));
            }
            Update::ResyncRequired => info.message(Severity::Info, "Refreshing application status"),
        })
    }
}

impl Info {
    fn status(&mut self, severity: Severity, text: &str) {
        self.host = Diagnostic {
            severity,
            text: clean(text),
        };
    }
    fn message(&mut self, severity: Severity, text: &str) {
        if self.messages.len() == 6 {
            self.messages.pop_front();
        }
        self.messages.push_back(Diagnostic {
            severity,
            text: clean(text),
        });
    }
    fn application<S>(&mut self, snapshot: &ApplicationSnapshot<S>) {
        let target = match snapshot.desired {
            LightingTarget::Off => "off",
            LightingTarget::Processed => "processed",
            LightingTarget::DeviceNative => "native",
        };
        let session = match &snapshot.session {
            SessionState::Idle => "idle",
            SessionState::Starting(_) => "starting",
            SessionState::Running(_) => "running",
            SessionState::Waiting(_) => "waiting",
            SessionState::Stopping(_) => "stopping",
            SessionState::Failed {
                cleanup_complete: true,
                ..
            } => "failed; cleanup complete",
            SessionState::Failed {
                cleanup_complete: false,
                ..
            } => "failed; cleanup unconfirmed",
        };
        self.application = format!("Session: {session}   Target: {target}");
        self.capture = match &snapshot.observed_capture {
            None => "Capture: unknown".into(),
            Some(CaptureState::Idle) => "Capture: idle".into(),
            Some(CaptureState::Starting(_)) => "Capture: opening".into(),
            Some(CaptureState::Available(_)) => "Capture: available".into(),
            Some(CaptureState::Waiting { reason, .. }) => {
                format!("Capture: waiting - {}", clean(reason))
            }
            Some(CaptureState::Stopping(_)) => "Capture: releasing".into(),
            Some(CaptureState::Failed { failure, .. }) => {
                format!("Capture: {}", clean(&failure.message))
            }
        };
        self.revisions = format!(
            "Revision configured / applied: {} / {}",
            snapshot
                .configured_revision
                .map_or("-".into(), |r| r.0.to_string()),
            snapshot
                .applied_revision
                .map_or("-".into(), |r| r.0.to_string())
        );
    }
}

fn clean(text: &str) -> String {
    text.chars()
        .take(240)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
