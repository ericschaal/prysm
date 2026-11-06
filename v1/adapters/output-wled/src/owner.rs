//! One serialized writer. Awaiting a call never transfers ownership to its caller.
use crate::{
    Command, WledOwner, WledSettings,
    mailbox::Mailbox,
    transport::{Mode, Wire},
};
use prysm_contract::{
    ControlError, Failure, OwnerStopped,
    identity::{SessionId, StreamEpoch},
    output::{IdleOutputMode, OutputApplied},
};
use prysm_core::EdgeColors;
use std::{future::Future, sync::Arc, time::Duration};
use tokio::time::{Instant, sleep_until};

const REFRESH: Duration = Duration::from_millis(500);
enum Display {
    Unknown,
    Configured,
    Applying,
    Held(EdgeColors),
    Streaming {
        epoch: StreamEpoch,
        last: EdgeColors,
    },
    Idle(IdleOutputMode),
}
impl Display {
    fn colors(&self) -> Option<&EdgeColors> {
        match self {
            Self::Held(colors) | Self::Streaming { last: colors, .. } => Some(colors),
            _ => None,
        }
    }
}
struct Writer<W> {
    wire: W,
    mailbox: Arc<Mailbox>,
    configured: bool,
    wire_mode: Option<Mode>,
    display: Display,
    refresh_at: Instant,
    failure: Option<Failure>,
    poisoned: bool,
}

pub(super) async fn run<W: Wire>(
    owner: WledOwner,
    wire: W,
    shutdown: impl Future<Output = ()> + Send,
) -> Result<OwnerStopped, ControlError> {
    let mailbox = owner.mailbox.clone();
    let running = drive(owner, wire);
    tokio::pin!(running);
    tokio::select! {
        biased;
        _ = shutdown => {
            mailbox.request_shutdown();
            running.await
        }
        result = &mut running => result,
    }
}

async fn drive<W: Wire>(mut owner: WledOwner, wire: W) -> Result<OwnerStopped, ControlError> {
    let mut writer = Writer {
        wire,
        mailbox: owner.mailbox.clone(),
        configured: false,
        wire_mode: None,
        display: Display::Unknown,
        refresh_at: Instant::now(),
        failure: None,
        poisoned: false,
    };
    let mut commands_live = true;
    while !owner.mailbox.stopping() {
        tokio::select! {
            biased;
            command = owner.commands.recv(), if commands_live => match command {
                Some(command) => writer.command(command).await,
                None => commands_live = false,
            },
            _ = sleep_until(writer.refresh_at), if writer.display.colors().is_some() => writer.refresh().await,
            _ = owner.mailbox.notified() => {
                if let Some(frame) = owner.mailbox.take() { writer.frame(frame.colors).await; }
            }
        }
    }
    owner.commands.close();
    while let Ok(command) = owner.commands.try_recv() {
        let reply = match command {
            Command::Configure(_, reply)
            | Command::Admit(_, reply)
            | Command::Hold(_, _, reply)
            | Command::Finish(_, _, reply) => reply,
        };
        let _ = reply.send(Err(crate::CLOSED));
    }
    writer.shutdown().await?;
    Ok(OwnerStopped)
}

impl<W: Wire> Writer<W> {
    fn publish(&self) {
        let mut state = Mailbox::unknown();
        match &self.display {
            Display::Held(_) => state.held = true,
            Display::Streaming { epoch, .. } => state.admitted = Some(*epoch),
            Display::Idle(mode) => state.idle_mode = Some(*mode),
            _ => {}
        }
        if !matches!(self.display, Display::Unknown | Display::Applying) {
            state.confirmation = Some(OutputApplied::Sent);
        }
        state.failure = self.failure.clone();
        self.mailbox.publish(state);
    }
    fn begin(&mut self) {
        self.display = Display::Applying;
        self.publish();
    }
    fn applied(&mut self, display: Display) -> OutputApplied {
        self.display = display;
        self.failure = None;
        self.refresh_at = Instant::now() + REFRESH;
        self.publish();
        OutputApplied::Sent
    }
    fn failed(&mut self, error: ControlError) -> ControlError {
        let failure = Failure::from(error);
        self.poisoned |= failure.kind == prysm_contract::FailureKind::RecoveryRequired;
        self.wire_mode = None;
        self.display = Display::Applying;
        self.failure = Some(failure.clone());
        self.mailbox.suspend();
        self.publish();
        failure.into()
    }
    fn ready(&self) -> Result<(), ControlError> {
        if self.poisoned {
            return Err(self
                .failure
                .clone()
                .expect("poisoned owner retains its failure")
                .into());
        }
        if !self.configured {
            return Err(ControlError::Unavailable(
                "configure WLED before controlling output",
            ));
        }
        Ok(())
    }
    async fn mode(&mut self, mode: Mode, terminal: bool) -> Result<(), ControlError> {
        for write in [Mode::Release, mode] {
            if !terminal {
                self.mailbox.authorize()?;
            }
            if let Err(error) = self.wire.mode(write).await {
                let error = if error.uncertain {
                    ControlError::RecoveryRequired(Box::new(error.error))
                } else {
                    error.error
                };
                return Err(self.failed(error));
            }
        }
        self.wire_mode = Some(mode);
        Ok(())
    }
    async fn realtime(&mut self) -> Result<(), ControlError> {
        if self.wire_mode != Some(Mode::Realtime) {
            self.mode(Mode::Realtime, false).await?;
        }
        Ok(())
    }
    async fn send(&mut self, colors: &EdgeColors) -> Result<(), ControlError> {
        self.mailbox.authorize()?;
        if let Err(error) = self.wire.colors(colors).await {
            return Err(self.failed(error));
        }
        self.refresh_at = Instant::now() + REFRESH;
        Ok(())
    }
    async fn command(&mut self, command: Command) {
        let (reply, result) = match command {
            Command::Configure(settings, reply) => (reply, self.configure(settings)),
            Command::Admit(epoch, reply) => (reply, self.admit(epoch).await),
            Command::Hold(epoch, colors, reply) => (reply, self.hold(epoch, colors).await),
            Command::Finish(session, mode, reply) => (reply, self.finish(session, mode).await),
        };
        let _ = reply.send(result);
    }
    fn configure(&mut self, settings: WledSettings) -> Result<OutputApplied, ControlError> {
        if self.poisoned {
            self.ready()?;
        }
        settings.validate()?;
        self.mailbox.can_configure()?;
        self.wire.configure(&settings)?;
        self.configured = true;
        self.wire_mode = None;
        Ok(self.applied(Display::Configured))
    }
    async fn hold(
        &mut self,
        epoch: StreamEpoch,
        colors: EdgeColors,
    ) -> Result<OutputApplied, ControlError> {
        self.ready()?;
        self.mailbox.hold(epoch)?;
        self.begin();
        self.realtime().await?;
        self.send(&colors).await?;
        Ok(self.applied(Display::Held(colors)))
    }
    async fn admit(&mut self, epoch: StreamEpoch) -> Result<OutputApplied, ControlError> {
        self.ready()?;
        self.mailbox.admit(epoch)?;
        let mut colors = match &self.display {
            Display::Held(colors) => colors.clone(),
            Display::Streaming {
                epoch: previous,
                last,
            } if *previous == epoch => last.clone(),
            _ => EdgeColors::default(),
        };
        self.begin();
        self.realtime().await?;
        if let Some(frame) = self.mailbox.take() {
            colors = frame.colors;
        }
        self.send(&colors).await?;
        Ok(self.applied(Display::Streaming {
            epoch,
            last: colors,
        }))
    }
    async fn finish(
        &mut self,
        session: SessionId,
        mode: IdleOutputMode,
    ) -> Result<OutputApplied, ControlError> {
        // Even an uncertain owner seals local offers; it cannot certify remote cleanup.
        if !self.configured {
            self.ready()?;
        }
        self.mailbox.finish(session)?;
        self.ready()?;
        self.begin();
        self.mode(
            match mode {
                IdleOutputMode::Off => Mode::Off,
                IdleOutputMode::Native => Mode::Native,
            },
            false,
        )
        .await?;
        self.mailbox.finished(session);
        Ok(self.applied(Display::Idle(mode)))
    }
    async fn frame(&mut self, colors: EdgeColors) {
        if let Display::Streaming { epoch, .. } = self.display
            && self.send(&colors).await.is_ok()
        {
            // Color-only updates do not change the public snapshot or flood observers.
            self.display = Display::Streaming {
                epoch,
                last: colors,
            };
        }
    }
    async fn refresh(&mut self) {
        if let Some(colors) = self.display.colors().cloned() {
            let _ = self.send(&colors).await;
        }
    }
    async fn shutdown(&mut self) -> Result<(), ControlError> {
        self.mailbox.suspend();
        if self.poisoned {
            return self.ready();
        }
        let prior = self.failure.clone();
        if self.configured && self.wire_mode != Some(Mode::Off) {
            self.begin();
            if let Err(cleanup) = self.mode(Mode::Off, true).await {
                return Err(match prior {
                    Some(prior) => {
                        let recovery =
                            cleanup.kind() == prysm_contract::FailureKind::RecoveryRequired;
                        let error = Box::new(ShutdownFailure { prior, cleanup });
                        if recovery {
                            ControlError::RecoveryRequired(error)
                        } else {
                            ControlError::Adapter(error)
                        }
                    }
                    None => cleanup,
                });
            }
            self.applied(Display::Idle(IdleOutputMode::Off));
        }
        prior.map_or(Ok(()), |failure| {
            self.failure = Some(failure.clone());
            self.publish();
            Err(failure.into())
        })
    }
}

#[derive(Debug)]
struct ShutdownFailure {
    prior: Failure,
    cleanup: ControlError,
}
impl std::fmt::Display for ShutdownFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}; output cleanup also failed: {}",
            self.prior, self.cleanup
        )
    }
}
impl std::error::Error for ShutdownFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cleanup)
    }
}
