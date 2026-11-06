//! Terminal output through the same control/color ports as a physical output.
mod diagnostics;
mod mailbox;
mod preview;
mod screen;
mod ui;
mod video;
mod worker;

pub use diagnostics::{HostDiagnostic, TerminalDiagnostics};
use mailbox::Mailbox;
use prysm_contract::{
    ControlError, OwnerStopped,
    identity::{InstanceId, SessionId, StreamEpoch},
    observation::Observation,
    output::{
        ColorFrame, ColorSink, IdleOutputMode, OfferResult, OutputApplied, OutputCapabilities,
        OutputControl, OutputState,
    },
    preview::{PreviewLease, PreviewSource},
};
use prysm_core::EdgeColors;
use std::{future::Future, sync::Arc};
use tokio::sync::{mpsc, oneshot};

const CAPACITY: usize = 16;
const CLOSED: ControlError = ControlError::Unavailable("terminal output is closed");
type Reply = oneshot::Sender<Result<OutputApplied, ControlError>>;
enum Command {
    Configure(TerminalSettings, Reply),
    Hold(StreamEpoch, EdgeColors, Reply),
    Admit(StreamEpoch, Reply),
    Finish(SessionId, IdleOutputMode, Reply),
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "wire", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "wire", serde(deny_unknown_fields))]
pub struct TerminalSettings {
    /// Top, right, bottom, left. Display cells sample these LED positions.
    pub leds: [u32; 4],
}
impl TerminalSettings {
    fn validate(&self) -> Result<(), ControlError> {
        let total: u64 = self.leds.iter().map(|count| u64::from(*count)).sum();
        if !(1..=65535).contains(&total) {
            return Err(ControlError::Invalid {
                field: "leds",
                reason: "total LED count must be between 1 and 65535".into(),
            });
        }
        Ok(())
    }
}

pub struct TerminalOutput {
    pub owner: TerminalOwner,
    pub control: TerminalControl,
    pub colors: TerminalColors,
    pub shutdown: TerminalShutdown,
    pub diagnostics: TerminalDiagnostics,
}
impl TerminalOutput {
    /// Inert construction. The owner acquires stdout only when run.
    pub fn new(instance: InstanceId) -> Self {
        let (mailbox, receiver) = Mailbox::new(instance);
        let mailbox = Arc::new(mailbox);
        Self {
            owner: TerminalOwner {
                commands: receiver,
                preview: None,
                mailbox: mailbox.clone(),
            },
            control: TerminalControl {
                mailbox: mailbox.clone(),
            },
            colors: TerminalColors(mailbox.clone()),
            shutdown: TerminalShutdown(mailbox.clone()),
            diagnostics: TerminalDiagnostics(mailbox),
        }
    }
}

pub struct TerminalOwner {
    preview: Option<PreviewLease>,
    commands: mpsc::Receiver<Command>,
    mailbox: Arc<Mailbox>,
}
impl TerminalOwner {
    /// Subscribe to optional diagnostic imagery without starting capture.
    /// A failed subscription disables preview only; lighting remains available.
    pub async fn with_preview(mut self, source: &impl PreviewSource) -> Self {
        match source.subscribe().await {
            Ok(lease) => {
                self.preview = Some(lease);
                self.mailbox.preview_waiting();
            }
            Err(error) => self.mailbox.disable_preview(error.to_string()),
        }
        self
    }

    /// Retain until completion. One joined thread owns screen writes/restoration;
    /// terminal I/O never runs on the host executor. No raw mode or input reader.
    pub async fn run(
        self,
        shutdown: impl Future<Output = ()> + Send,
    ) -> Result<OwnerStopped, ControlError> {
        worker::run(self, shutdown, screen::Screen::open).await
    }
}
impl Drop for TerminalOwner {
    fn drop(&mut self) {
        self.mailbox.close();
    }
}

#[derive(Clone)]
pub struct TerminalShutdown(Arc<Mailbox>);
impl TerminalShutdown {
    /// Close admission immediately; only owner completion certifies restoration.
    pub fn request(&self) {
        self.0.request_shutdown();
    }
}

#[derive(Clone)]
pub struct TerminalControl {
    mailbox: Arc<Mailbox>,
}
impl TerminalControl {
    async fn request(
        &self,
        command: impl FnOnce(Reply) -> Command,
    ) -> Result<OutputApplied, ControlError> {
        let (reply, response) = oneshot::channel();
        self.mailbox.send(command(reply)).await?;
        response.await.map_err(|_| CLOSED)?
    }
}
impl OutputControl for TerminalControl {
    type Settings = TerminalSettings;
    fn capabilities(&self) -> OutputCapabilities {
        OutputCapabilities { native_mode: false }
    }
    fn validate(&self, settings: &TerminalSettings) -> Result<(), ControlError> {
        settings.validate()
    }
    async fn configure(&self, settings: TerminalSettings) -> Result<OutputApplied, ControlError> {
        settings.validate()?;
        self.request(|reply| Command::Configure(settings, reply))
            .await
    }
    async fn hold(
        &self,
        epoch: StreamEpoch,
        colors: EdgeColors,
    ) -> Result<OutputApplied, ControlError> {
        self.request(|reply| Command::Hold(epoch, colors, reply))
            .await
    }
    async fn admit(&self, epoch: StreamEpoch) -> Result<OutputApplied, ControlError> {
        self.request(|reply| Command::Admit(epoch, reply)).await
    }
    async fn finish(
        &self,
        session: SessionId,
        mode: IdleOutputMode,
    ) -> Result<OutputApplied, ControlError> {
        if mode == IdleOutputMode::Native {
            return Err(ControlError::Unavailable(
                "terminal output has no native mode",
            ));
        }
        self.request(|reply| Command::Finish(session, mode, reply))
            .await
    }
    async fn observe(&self) -> Result<Observation<OutputState>, ControlError> {
        self.mailbox.observe()
    }
}
#[derive(Clone)]
pub struct TerminalColors(Arc<Mailbox>);
impl ColorSink for TerminalColors {
    fn offer(&self, frame: ColorFrame) -> Result<OfferResult, ControlError> {
        self.0.offer(frame)
    }
}

#[cfg(test)]
mod tests;
