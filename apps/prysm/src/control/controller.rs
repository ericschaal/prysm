//! One controller owns settings, configured resources, and accepted operations.
use super::ready::{OperationResult, Ready, failed_state};
use crate::{
    preview::Publisher,
    settings::{SettingsError, SettingsStore},
};
use anyhow::{Context as _, Result};
use prysm_api::{App, Command, CommandError};
use prysm_contract::{
    Config, OperationId, OperationResponse, OperationStatus, SettingsView, Source, StateResponse,
};
use std::time::Duration;
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

type SettingsReply = oneshot::Sender<Result<SettingsView, CommandError>>;

enum Mode {
    Setup,
    Ready(Box<Ready>),
    Failed(Box<StateResponse>),
}

impl Mode {
    fn configured(
        config: &Config,
        preview: Publisher,
        receiver_listen: std::net::SocketAddr,
    ) -> Self {
        match Ready::new(config, preview, receiver_listen) {
            Ok(ready) => Self::Ready(Box::new(ready)),
            Err(error) => Self::Failed(Box::new(failed_state(config, &error))),
        }
    }

    fn state(&self, operation: OperationStatus) -> Option<StateResponse> {
        match self {
            Self::Setup => None,
            Self::Ready(ready) => Some(ready.state(operation)),
            Self::Failed(state) => {
                let mut state = state.as_ref().clone();
                state.operation = operation;
                Some(state)
            }
        }
    }

    async fn observe(&mut self) -> Result<()> {
        match self {
            Self::Ready(ready) => ready.observe().await,
            Self::Setup | Self::Failed(_) => std::future::pending().await,
        }
    }

    async fn shutdown(self) -> Result<()> {
        match self {
            Self::Ready(ready) => ready.shutdown().await,
            Self::Setup | Self::Failed(_) => Ok(()),
        }
    }
}

enum Finished {
    Applied {
        id: OperationId,
        operation: OperationResult,
    },
    Switched {
        id: OperationId,
        source: Source,
        operation: OperationResult,
    },
    Reset {
        id: OperationId,
        result: Result<()>,
        state: Box<StateResponse>,
        reply: SettingsReply,
    },
}

pub(crate) struct Controller {
    settings: SettingsStore,
    mode: Mode,
    preview: Publisher,
    receiver_listen: std::net::SocketAddr,
    operation: OperationStatus,
    job: Option<JoinHandle<Finished>>,
    settings_tx: watch::Sender<SettingsView>,
    state_tx: watch::Sender<Option<StateResponse>>,
}

pub(crate) struct ControllerHandle {
    shutdown: CancellationToken,
    task: JoinHandle<Result<()>>,
}

impl ControllerHandle {
    pub async fn wait(&mut self) -> Result<()> {
        (&mut self.task).await.context("controller task panicked")?
    }

    pub async fn shutdown(self) -> Result<()> {
        self.shutdown.cancel();
        self.task.await.context("controller task panicked")?
    }
}

impl Controller {
    pub fn start(
        settings: SettingsStore,
        preview: Publisher,
        receiver_listen: std::net::SocketAddr,
    ) -> (ControllerHandle, App) {
        let view = settings.view();
        let mode = settings.config().map_or(Mode::Setup, |config| {
            Mode::configured(&config, preview.clone(), receiver_listen)
        });
        let (settings_tx, settings_rx) = watch::channel(view.clone());
        let (state_tx, state_rx) = watch::channel(mode.state(OperationStatus::Idle));
        let (commands, receiver) = mpsc::channel(32);
        let shutdown = CancellationToken::new();
        let controller = Self {
            settings,
            mode,
            preview,
            receiver_listen,
            operation: OperationStatus::Idle,
            job: None,
            settings_tx,
            state_tx,
        };
        let task = tokio::spawn(controller.run(receiver, shutdown.clone()));
        (
            ControllerHandle { shutdown, task },
            App::new(commands, settings_rx, state_rx),
        )
    }

    fn publish(&self) {
        let view = self.settings.view();
        self.state_tx
            .send_replace(self.mode.state(self.operation.clone()));
        self.settings_tx.send_replace(view);
    }

    fn begin(&mut self) -> OperationId {
        let id = OperationId::new();
        self.operation = OperationStatus::Pending { id };
        self.publish();
        id
    }

    fn complete(&mut self, id: OperationId, result: Result<(), CommandError>) {
        self.operation = match result {
            Ok(()) => OperationStatus::Succeeded { id },
            Err(error) => {
                tracing::error!(operation_id=?id, error=%error, "operation failed");
                OperationStatus::Failed {
                    id,
                    error: error.report(),
                }
            }
        };
        self.publish();
    }

    async fn accept(&mut self, command: Command) {
        if self.job.is_some() {
            Self::reject(
                command,
                CommandError::Conflict("a change is already running"),
            );
            return;
        }
        match command {
            Command::UpdateSettings(update, reply) => match self.settings.update(*update).await {
                Ok(_) => {
                    let id = self.begin();
                    self.reconcile(id, reply);
                }
                Err(error) => {
                    let _ = reply.send(Err(error.into()));
                }
            },
            Command::SetTarget(target, reply) => {
                let operation = match &self.mode {
                    Mode::Ready(ready) => ready.switch(target),
                    Mode::Setup => {
                        let _ = reply.send(Err(CommandError::Conflict(
                            "complete settings before controlling the lights",
                        )));
                        return;
                    }
                    Mode::Failed(state) => {
                        let _ = reply.send(Err(CommandError::Backend(anyhow::anyhow!(
                            state.pipeline.error.clone().unwrap_or_default()
                        ))));
                        return;
                    }
                };
                let id = self.begin();
                self.job = Some(tokio::spawn(async move {
                    Finished::Switched {
                        id,
                        source: target.source,
                        operation: operation.await,
                    }
                }));
                let _ = reply.send(Ok(OperationResponse { operation_id: id }));
            }
        }
    }

    fn reconcile(&mut self, id: OperationId, reply: SettingsReply) {
        let view = self.settings.view();
        let previous = std::mem::replace(&mut self.mode, Mode::Setup);
        match (previous, self.settings.config()) {
            (Mode::Ready(mut ready), Some(config)) => {
                let operation = ready.apply(config);
                self.mode = Mode::Ready(ready);
                self.job = Some(tokio::spawn(async move {
                    Finished::Applied {
                        id,
                        operation: operation.await,
                    }
                }));
                self.publish();
                let _ = reply.send(Ok(view));
            }
            (Mode::Ready(ready), None) => {
                let state = Box::new(ready.state(self.operation.clone()));
                self.job = Some(tokio::spawn(async move {
                    Finished::Reset {
                        id,
                        result: ready.shutdown().await,
                        state,
                        reply,
                    }
                }));
                self.publish();
            }
            (_, Some(config)) => {
                self.mode = Mode::configured(&config, self.preview.clone(), self.receiver_listen);
                let result = match &self.mode {
                    Mode::Failed(state) => Err(CommandError::Backend(anyhow::anyhow!(
                        state.pipeline.error.clone().unwrap_or_default()
                    ))),
                    _ => Ok(()),
                };
                self.complete(id, result);
                let _ = reply.send(Ok(view));
            }
            (_, None) => {
                self.complete(id, Ok(()));
                let _ = reply.send(Ok(view));
            }
        }
    }

    fn finish_operation(&mut self, id: OperationId, operation: OperationResult) {
        let OperationResult {
            result,
            observation,
        } = operation;
        if let Mode::Ready(ready) = &mut self.mode {
            ready.finish(observation);
        }
        self.complete(id, result);
    }

    async fn finished(&mut self, finished: Finished) {
        match finished {
            Finished::Applied { id, operation } => self.finish_operation(id, operation),
            Finished::Switched {
                id,
                source,
                mut operation,
            } => {
                if operation.result.is_ok() {
                    operation.result = match self.settings.set_source(source).await {
                        Ok(Some(config)) => match &mut self.mode {
                            Mode::Ready(ready) => ready.source_saved(config).await,
                            _ => unreachable!(
                                "accepted lighting operation retains its configured resources"
                            ),
                        },
                        Ok(None) => Ok(()),
                        Err(error) => Err(CommandError::Persistence(
                            anyhow::Error::new(error)
                                .context("lights changed but source could not be saved"),
                        )),
                    };
                }
                self.finish_operation(id, operation);
            }
            Finished::Reset {
                id,
                result,
                mut state,
                reply,
            } => match result {
                Ok(()) => {
                    self.complete(id, Ok(()));
                    let _ = reply.send(Ok(self.settings.view()));
                }
                Err(error) => {
                    let error = CommandError::Backend(
                        error.context("settings saved but runtime shutdown failed"),
                    );
                    state.config_revision = self.settings.view().revision;
                    state.pipeline.phase = prysm_contract::PipelinePhase::Failed;
                    state.pipeline.error = Some(error.to_string());
                    state.observed = None;
                    self.mode = Mode::Failed(state);
                    self.operation = OperationStatus::Failed {
                        id,
                        error: error.report(),
                    };
                    self.publish();
                    let _ = reply.send(Err(error));
                }
            },
        }
    }

    async fn commands(
        &mut self,
        receiver: &mut mpsc::Receiver<Command>,
        shutdown: &CancellationToken,
    ) -> Result<()> {
        let mut draining = false;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            if !draining && shutdown.is_cancelled() {
                draining = true;
                receiver.close();
            }
            if draining && self.job.is_none() {
                return Ok(());
            }
            tokio::select! {
                () = shutdown.cancelled(), if !draining => {},
                finished = async {
                    match self.job.as_mut() {
                        Some(job) => job.await,
                        None => std::future::pending().await,
                    }
                } => {
                    self.job = None;
                    self.finished(finished.context("accepted operation task panicked")?).await;
                }
                update = self.mode.observe() => { update?; self.publish(); }
                command = receiver.recv(), if !draining => match command {
                    Some(command) => self.accept(command).await,
                    None => draining = true,
                },
                _ = tick.tick(), if !draining => self.publish(),
            }
        }
    }

    async fn run(
        mut self,
        mut receiver: mpsc::Receiver<Command>,
        shutdown: CancellationToken,
    ) -> Result<()> {
        let mut result = self.commands(&mut receiver, &shutdown).await;
        receiver.close();
        while let Ok(command) = receiver.try_recv() {
            Self::reject(command, CommandError::Unavailable);
        }
        while let Some(job) = self.job.take() {
            match job.await {
                Ok(finished) => self.finished(finished).await,
                Err(error) => {
                    if result.is_ok() {
                        result = Err(error).context("accepted operation task panicked");
                    }
                    break;
                }
            }
        }
        let stopped = self.mode.shutdown().await;
        result?;
        stopped
    }

    fn reject(command: Command, error: CommandError) {
        match command {
            Command::UpdateSettings(_, reply) => {
                let _ = reply.send(Err(error));
            }
            Command::SetTarget(_, reply) => {
                let _ = reply.send(Err(error));
            }
        }
    }
}

impl From<SettingsError> for CommandError {
    fn from(error: SettingsError) -> Self {
        match error {
            SettingsError::Conflict(message) => Self::Conflict(message),
            SettingsError::Persistence(error) => Self::Persistence(error),
            SettingsError::Invalid(error) => Self::Invalid(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{preview::Preview, test_support};

    fn owner(
        settings: SettingsStore,
        receiver_listen: std::net::SocketAddr,
    ) -> (Controller, App, mpsc::Receiver<Command>) {
        let preview = Preview::new().publisher();
        let view = settings.view();
        let mode = settings.config().map_or(Mode::Setup, |config| {
            Mode::configured(&config, preview.clone(), receiver_listen)
        });
        let (settings_tx, settings_rx) = watch::channel(view.clone());
        let (state_tx, state_rx) = watch::channel(mode.state(OperationStatus::Idle));
        let (commands, receiver) = mpsc::channel(32);
        (
            Controller {
                settings,
                mode,
                preview,
                receiver_listen,
                operation: OperationStatus::Idle,
                job: None,
                settings_tx,
                state_tx,
            },
            App::new(commands, settings_rx, state_rx),
            receiver,
        )
    }

    #[tokio::test]
    async fn accepted_task_panic_still_joins_owned_receiver() {
        let directory = tempfile::tempdir().unwrap();
        let available = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = available.local_addr().unwrap();
        drop(available);
        let mut config = test_support::config();
        config.input = prysm_contract::Input::Webos {
            capture_resolution: [4, 2].try_into().unwrap(),
            refresh_rate_hz: 30.try_into().unwrap(),
        };
        let (mut controller, _app, receiver) =
            owner(test_support::store(&directory, &config), address);
        controller.job = Some(tokio::spawn(async {
            panic!("accepted operation failed unexpectedly")
        }));
        assert!(std::net::TcpListener::bind(address).is_err());
        let error = controller
            .run(receiver, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("accepted operation task panicked"));
        assert!(
            std::net::TcpListener::bind(address).is_ok(),
            "error cleanup must join the receiver before returning"
        );
    }

    #[tokio::test]
    async fn failed_reset_reports_saved_settings_and_failed_cleanup() {
        let directory = tempfile::tempdir().unwrap();
        let config = test_support::config();
        let mut settings = test_support::store(&directory, &config);
        let mut incomplete = settings.view().settings;
        incomplete.input = None;
        settings
            .update(
                serde_json::from_value(serde_json::json!({
                    "expected_revision":settings.view().revision,"settings": incomplete
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let (mut controller, app, _receiver) = owner(settings, "127.0.0.1:0".parse().unwrap());
        let (reply, response) = oneshot::channel();
        let id = controller.begin();
        controller
            .finished(Finished::Reset {
                id,
                reply,
                result: Err(anyhow::anyhow!("native cleanup failed")),
                state: Box::new(failed_state(&config, &anyhow::anyhow!("previous failure"))),
            })
            .await;
        let error = response.await.unwrap().unwrap_err();
        assert!(matches!(error, CommandError::Backend(_)));
        assert!(
            error
                .to_string()
                .contains("settings saved but runtime shutdown failed")
        );
        assert_eq!(
            app.settings().missing_required,
            [prysm_contract::RequiredSetting::Input]
        );
        let state = app.state().unwrap();
        assert!(matches!(state.operation,OperationStatus::Failed {id:failed,..} if failed==id));
        assert!(
            state
                .pipeline
                .error
                .unwrap()
                .contains("native cleanup failed")
        );
    }
}
