//! One controller owns settings, configured resources, and accepted operations.
use super::calibration::{Effect, LOCKED, Session};
use super::ready::{OperationResult, Ready, failed_state};
use crate::{
    preview::Publisher,
    settings::{SettingsError, SettingsStore},
};
use anyhow::{Context as _, Result};
use prysm_api::{App, Command, CommandError};
use prysm_contract::{CalibrationRequest, CalibrationView};
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
        webos: prysm_capture::WebosReceiver,
    ) -> Self {
        match Ready::new(config, preview, webos) {
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
    CalibrationStarted(OperationResult),
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
    calibration: Option<Session>,
    settings: SettingsStore,
    mode: Mode,
    preview: Publisher,
    webos: prysm_capture::WebosReceiver,
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
        webos: prysm_capture::WebosReceiver,
    ) -> (ControllerHandle, App) {
        let view = settings.view();
        let mode = settings.config().map_or(Mode::Setup, |config| {
            Mode::configured(&config, preview.clone(), webos.clone())
        });
        let (settings_tx, settings_rx) = watch::channel(view.clone());
        let (state_tx, state_rx) = watch::channel(mode.state(OperationStatus::Idle));
        let (commands, receiver) = mpsc::channel(32);
        let shutdown = CancellationToken::new();
        let controller = Self {
            calibration: None,
            settings,
            mode,
            preview,
            webos,
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
        let mut state = self.mode.state(self.operation.clone());
        if let Some(state) = &mut state {
            state.calibration_active = self.calibration.as_ref().is_some_and(Session::active);
        }
        self.state_tx.send_replace(state);
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
        if let Command::Calibrate(request, reply) = command {
            let result = self.calibrate(request).await;
            let _ = reply.send(result);
            self.publish();
            return;
        }
        if self.calibration.as_ref().is_some_and(Session::active) {
            if matches!(&command, Command::SetTarget(target, _) if !target.power) {
                if let Err(error) = self
                    .end_calibration("Lighting stopped. Unsaved calibration discarded.", true)
                    .await
                {
                    tracing::error!(%error, "could not restore calibration settings before Stop");
                }
            } else {
                Self::reject(command, CommandError::Conflict(LOCKED));
                return;
            }
        }
        if self.job.is_some() {
            Self::reject(
                command,
                CommandError::Conflict("a change is already running"),
            );
            return;
        }
        match command {
            Command::Calibrate(_, _) => unreachable!("handled above"),
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

    async fn calibrate(
        &mut self,
        request: CalibrationRequest,
    ) -> Result<CalibrationView, CommandError> {
        use std::time::Instant;
        let now = Instant::now();
        if matches!(request, CalibrationRequest::Begin) {
            if self.job.is_some() || self.calibration.as_ref().is_some_and(Session::active) {
                return Err(CommandError::Conflict(LOCKED));
            }
            let config = self.settings.config().ok_or(CommandError::Conflict(
                "complete settings before calibration",
            ))?;
            if !matches!(config.input, prysm_contract::Input::Webos { .. }) {
                return Err(CommandError::Conflict(
                    "calibration needs the webOS capture input",
                ));
            }
            if !matches!(self.mode, Mode::Ready(_)) {
                return Err(CommandError::Conflict("capture is unavailable"));
            }
            let session = Session::new(self.settings.view().settings.calibration.profiles, now);
            let view = session.tv_view(now);
            self.calibration = Some(session);
            return Ok(view);
        }
        let session = self.calibration.as_mut().ok_or(CommandError::Conflict(
            "calibration has ended; start again on the TV",
        ))?;
        let (token, effect) = session.transition(request, now)?;
        match effect {
            Effect::None => {}
            Effect::Start => {
                self.apply_calibration().await?;
                let Mode::Ready(ready) = &self.mode else {
                    return Err(CommandError::Unavailable);
                };
                let operation =
                    ready.start_calibration(self.calibration.as_ref().unwrap().startup_cancel());
                self.job = Some(tokio::spawn(async move {
                    Finished::CalibrationStarted(operation.await)
                }));
            }
            Effect::Save(profile) => {
                if self.job.is_some()
                    || !self
                        .mode
                        .state(self.operation.clone())
                        .is_some_and(|s| s.pipeline.phase == prysm_contract::PipelinePhase::Running)
                {
                    return Err(CommandError::CapturePending);
                }
                self.settings.save_profile(&profile).await?;
                self.calibration.as_mut().unwrap().saved(profile);
                self.end_calibration("Calibration saved and active.", false)
                    .await?;
            }
            Effect::End(message) => self.end_calibration(message, true).await?,
        }
        self.apply_calibration().await?;
        Ok(self
            .calibration
            .as_ref()
            .unwrap()
            .view(token, Instant::now(), self.job.is_some()))
    }

    async fn apply_calibration(&mut self) -> Result<(), CommandError> {
        let Some(session) = &mut self.calibration else {
            return Ok(());
        };
        if !session.active() || !session.started() || self.job.is_some() {
            return Ok(());
        }
        let normal = self.settings.config().ok_or(CommandError::Unavailable)?;
        let Some(effective) = session.pending_config(normal, std::time::Instant::now()) else {
            return Ok(());
        };
        let Mode::Ready(ready) = &mut self.mode else {
            return Err(CommandError::Unavailable);
        };
        ready.live(effective.clone()).await?;
        session.mark_applied(effective);
        Ok(())
    }

    async fn end_calibration(&mut self, message: &str, stop: bool) -> Result<(), CommandError> {
        if let Some(session) = &mut self.calibration {
            session.finish(message);
        }
        // Active calibration excludes other jobs; join startup before stopping its output.
        let joined = match self.job.take() {
            Some(job) => job.await.map(|_| ()).map_err(|error| {
                CommandError::Backend(
                    anyhow::Error::new(error).context("calibration startup task panicked"),
                )
            }),
            None => Ok(()),
        };
        let normal = self.settings.config().ok_or(CommandError::Unavailable)?;
        let Mode::Ready(ready) = &mut self.mode else {
            return Err(CommandError::Unavailable);
        };
        // Stop first so restoring settings can never restart a failed or cancelled capture.
        let stopped = if stop {
            let operation = ready
                .switch(prysm_contract::Target {
                    power: false,
                    source: normal.source,
                })
                .await;
            ready.finish(operation.observation);
            operation.result
        } else {
            Ok(())
        };
        let restored = ready.live(normal).await;
        let result = joined.and(stopped).and(restored);
        if result.is_err()
            && let Some(session) = &mut self.calibration
        {
            session.finish("Calibration ended, but runtime cleanup failed. Check the light status before continuing.");
        }
        result
    }

    async fn calibration_tick(&mut self) {
        let Some(session) = &self.calibration else {
            return;
        };
        if !session.active() {
            return;
        }
        let failed = session.started()
            && self.job.is_none()
            && self.mode.state(self.operation.clone()).is_none_or(|s| {
                matches!(
                    s.pipeline.phase,
                    prysm_contract::PipelinePhase::Failed
                        | prysm_contract::PipelinePhase::Completed
                        | prysm_contract::PipelinePhase::Idle
                )
            });
        let result = if session.expired(std::time::Instant::now()) || failed {
            self.end_calibration(
                if failed {
                    "Calibration ended because capture stopped."
                } else {
                    "Calibration ended after the connection was lost."
                },
                true,
            )
            .await
        } else {
            self.apply_calibration().await
        };
        if let Err(error) = result {
            tracing::error!(%error, "calibration cleanup or runtime update failed");
            if self.calibration.as_ref().is_some_and(Session::active) {
                let _ = self
                    .end_calibration(
                        "Calibration ended because the runtime could not apply settings.",
                        true,
                    )
                    .await;
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
                self.mode = Mode::configured(&config, self.preview.clone(), self.webos.clone());
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
            Finished::CalibrationStarted(operation) => {
                if let Mode::Ready(ready) = &mut self.mode {
                    ready.finish(operation.observation);
                }
                let result = async {
                    operation.result?;
                    self.settings.set_source(Source::Prysm).await?;
                    self.apply_calibration().await
                }
                .await;
                if let Err(error) = result {
                    tracing::error!(%error, "calibration startup failed");
                    if let Err(error) = self.end_calibration(
                        "Calibration ended because startup failed. Check capture, WLED and settings storage.",
                        true,
                    ).await {
                        tracing::error!(%error, "calibration cleanup failed");
                    }
                }
                self.publish();
            }
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
                if let Some(session) = &self.calibration {
                    session.cancel_startup();
                }
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
                _ = tick.tick(), if !draining => { self.calibration_tick().await; self.publish(); },
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
            Command::Calibrate(_, reply) => {
                let _ = reply.send(Err(error));
            }
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
    use prysm_contract::CalibrationStep;

    fn owner(
        settings: SettingsStore,
        webos: prysm_capture::WebosReceiver,
    ) -> (Controller, App, mpsc::Receiver<Command>) {
        let preview = Preview::new().publisher();
        let view = settings.view();
        let mode = settings.config().map_or(Mode::Setup, |config| {
            Mode::configured(&config, preview.clone(), webos.clone())
        });
        let (settings_tx, settings_rx) = watch::channel(view.clone());
        let (state_tx, state_rx) = watch::channel(mode.state(OperationStatus::Idle));
        let (commands, receiver) = mpsc::channel(32);
        (
            Controller {
                calibration: None,
                settings,
                mode,
                preview,
                webos,
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
    async fn accepted_task_panic_still_reports_failure_after_cleanup() {
        let directory = tempfile::tempdir().unwrap();
        let webos = prysm_capture::WebosReceiver::new().unwrap();
        let mut config = test_support::config();
        config.input = prysm_contract::Input::Webos {
            capture_resolution: [4, 2].try_into().unwrap(),
            refresh_rate_hz: 30.try_into().unwrap(),
        };
        let (mut controller, _app, receiver) =
            owner(test_support::store(&directory, &config), webos);
        controller.job = Some(tokio::spawn(async {
            panic!("accepted operation failed unexpectedly")
        }));
        let error = controller
            .run(receiver, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("accepted operation task panicked"));
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
        let (mut controller, app, _receiver) =
            owner(settings, prysm_capture::WebosReceiver::new().unwrap());
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
    async fn calibration_owner(
        lighting_on: bool,
    ) -> (
        tempfile::TempDir,
        Controller,
        App,
        mpsc::Receiver<Command>,
        tokio::task::JoinHandle<()>,
    ) {
        use axum::{
            Json, Router,
            routing::{get, post},
        };
        let router = Router::new().route("/json/si", get(move || async move { Json(serde_json::json!({"state":{"on":lighting_on,"lor":if lighting_on {0} else {2}},"info":{"live":lighting_on,"lm":null}})) }))
            .route("/json/state", post(|| async { Json(serde_json::json!({"success":true})) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = test_support::config();
        config.input = prysm_contract::Input::Webos {
            capture_resolution: [4, 2].try_into().unwrap(),
            refresh_rate_hz: 30.try_into().unwrap(),
        };
        let prysm_contract::Output::Wled { url, .. } = &mut config.output;
        *url = format!("http://{}/", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await;
        });
        let directory = tempfile::tempdir().unwrap();
        let webos = prysm_capture::WebosReceiver::new().unwrap();
        let (controller, app, receiver) = owner(test_support::store(&directory, &config), webos);
        (directory, controller, app, receiver, server)
    }

    #[tokio::test]
    async fn calibration_startup_keeps_poll_and_stop_responsive() {
        let (_directory, mut controller, app, receiver, server) = calibration_owner(false).await;
        let normal = controller.settings.view();
        let (tv, phone) = paired(&mut controller).await;
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(controller.run(receiver, shutdown.clone()));
        let started = tokio::time::timeout(
            Duration::from_secs(1),
            app.calibrate(CalibrationRequest::Poll {
                token: tv.token,
                compare: false,
                playing: true,
            }),
        )
        .await
        .expect("startup must not block the controller")
        .unwrap();
        assert!(started.ended.is_none());
        tokio::time::timeout(Duration::from_secs(2), async {
            while app.state().unwrap().pipeline.phase != prysm_contract::PipelinePhase::Starting {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let polled = tokio::time::timeout(
            Duration::from_secs(1),
            app.calibrate(CalibrationRequest::Poll {
                token: phone.token,
                compare: false,
                playing: false,
            }),
        )
        .await
        .expect("phone heartbeats must work during startup")
        .unwrap();
        assert!(polled.ended.is_none());
        tokio::time::timeout(
            Duration::from_secs(1),
            app.set_target(prysm_contract::Target {
                power: false,
                source: normal.settings.source,
            }),
        )
        .await
        .expect("Stop must cancel unconfirmed startup")
        .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while matches!(
                app.state().unwrap().operation,
                OperationStatus::Pending { .. }
            ) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let state = app.state().unwrap();
        assert!(!state.calibration_active);
        assert_eq!(state.pipeline.phase, prysm_contract::PipelinePhase::Idle);
        assert_eq!(app.settings().settings, normal.settings);
        let ended = app
            .calibrate(CalibrationRequest::Poll {
                token: phone.token,
                compare: false,
                playing: false,
            })
            .await
            .unwrap();
        assert!(ended.ended.unwrap().contains("Lighting stopped"));
        shutdown.cancel();
        task.await.unwrap().unwrap();
        server.abort();
    }

    async fn paired(controller: &mut Controller) -> (CalibrationView, CalibrationView) {
        let tv = controller
            .calibrate(CalibrationRequest::Begin)
            .await
            .unwrap();
        let phone = controller
            .calibrate(CalibrationRequest::Pair {
                pairing: tv.pairing.unwrap(),
            })
            .await
            .unwrap();
        (tv, phone)
    }

    async fn tv_frame(receiver: &prysm_capture::WebosReceiver) -> crate::test_support::Tv {
        use futures::{SinkExt, StreamExt};
        tokio::time::timeout(Duration::from_secs(2), async {
            let mut sender = crate::test_support::tv_socket(receiver).await;
            while prysm_webos_protocol::ControlMessage::decode(
                &sender.next().await.unwrap().unwrap().into_data(),
            )
            .unwrap()
                == prysm_webos_protocol::ControlMessage::Idle
            {}
            let mut frame = prysm_webos_protocol::Header {
                width: 4,
                height: 2,
                format: prysm_webos_protocol::PixelFormat::Rgb,
                codec: prysm_webos_protocol::Codec::Raw,
                payload_len: 24,
            }
            .encode()
            .unwrap()
            .to_vec();
            frame.extend([128; 24]);
            sender
                .send(tokio_tungstenite::tungstenite::Message::binary(frame))
                .await
                .unwrap();
            sender
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "keep the real startup, queued draft and save scenario together"
    )]
    async fn calibration_startup_applies_queued_draft_before_save() {
        let (_directory, mut controller, app, receiver, server) = calibration_owner(true).await;
        let normal = app.settings().settings;
        let tv_receiver = controller.webos.clone();
        let (tv, phone) = paired(&mut controller).await;
        assert!(!tv.startup_pending);
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(controller.run(receiver, shutdown.clone()));
        app.calibrate(CalibrationRequest::Poll {
            token: tv.token,
            compare: false,
            playing: true,
        })
        .await
        .unwrap();
        let correction = prysm_contract::ColorCorrection {
            warmth: 40.try_into().unwrap(),
            ..prysm_contract::ColorCorrection::default()
        };
        let review = app
            .calibrate(CalibrationRequest::Update {
                token: phone.token,
                sequence: 1,
                correction,
                step: CalibrationStep::Review,
                shade: 0,
                paused: false,
                compare: false,
            })
            .await
            .unwrap();
        assert!(review.startup_pending);
        assert!(matches!(
            app.calibrate(CalibrationRequest::Save {
                token: phone.token,
                name: "Too early".into()
            })
            .await,
            Err(CommandError::CapturePending)
        ));
        let sender = tv_frame(&tv_receiver).await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while app.settings().settings.source != Source::Prysm {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("startup completion must persist the source");
        let running = app
            .calibrate(CalibrationRequest::Poll {
                token: tv.token,
                playing: true,
                compare: false,
            })
            .await
            .unwrap();
        assert!(!running.startup_pending);
        drop(sender);
        tokio::time::timeout(Duration::from_secs(2), async {
            while app.state().unwrap().pipeline.phase != prysm_contract::PipelinePhase::Starting {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(matches!(
            app.calibrate(CalibrationRequest::Save {
                token: phone.token,
                name: "Disconnected".into(),
            })
            .await,
            Err(CommandError::CapturePending)
        ));
        assert_eq!(app.settings().settings.calibration, normal.calibration);
        let _sender = tv_frame(&tv_receiver).await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while app.state().unwrap().pipeline.phase != prysm_contract::PipelinePhase::Running {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let saved = app
            .calibrate(CalibrationRequest::Save {
                token: phone.token,
                name: "Queued draft".into(),
            })
            .await
            .unwrap();
        assert!(saved.saved);
        assert_eq!(app.settings().settings.calibration.correction(), correction);
        assert_eq!(app.settings().settings.processing, normal.processing);
        assert_eq!(
            app.state().unwrap().pipeline.phase,
            prysm_contract::PipelinePhase::Running
        );
        shutdown.cancel();
        task.await.unwrap().unwrap();
        server.abort();
    }

    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "one session exercises pairing, locking, ordering and exit together"
    )]
    async fn calibration_pairing_lock_stale_updates_and_tv_exit() {
        let (directory, mut controller, _, _receiver, server) = calibration_owner(false).await;
        let before = controller.settings.view();
        let bytes = std::fs::read(directory.path().join("settings.json")).unwrap();
        let (tv, phone) = paired(&mut controller).await;
        assert!(
            controller
                .calibrate(CalibrationRequest::Pair {
                    pairing: tv.pairing.unwrap()
                })
                .await
                .is_err()
        );
        assert!(
            controller
                .calibrate(CalibrationRequest::Poll {
                    token: OperationId::new(),
                    compare: false,
                    playing: false
                })
                .await
                .is_err()
        );
        let (reply, result) = oneshot::channel();
        controller
            .accept(Command::UpdateSettings(
                Box::new(prysm_contract::UpdateSettings {
                    expected_revision: before.revision,
                    settings: before.settings.clone(),
                }),
                reply,
            ))
            .await;
        assert!(matches!(
            result.await.unwrap(),
            Err(CommandError::Conflict(LOCKED))
        ));
        let (reply, result) = oneshot::channel();
        controller
            .accept(Command::SetTarget(
                prysm_contract::Target {
                    power: true,
                    source: Source::Wled,
                },
                reply,
            ))
            .await;
        assert!(matches!(
            result.await.unwrap(),
            Err(CommandError::Conflict(LOCKED))
        ));
        let correction = prysm_contract::ColorCorrection {
            warmth: 40.try_into().unwrap(),
            ..prysm_contract::ColorCorrection::default()
        };
        for (sequence, value) in [
            (2, correction),
            (1, prysm_contract::ColorCorrection::default()),
        ] {
            controller
                .calibrate(CalibrationRequest::Update {
                    token: phone.token,
                    sequence,
                    correction: value,
                    step: CalibrationStep::WhiteBalance,
                    shade: 0,
                    paused: false,
                    compare: false,
                })
                .await
                .unwrap();
        }
        assert_eq!(
            controller
                .calibration
                .as_ref()
                .unwrap()
                .view(phone.token, std::time::Instant::now(), false)
                .draft
                .correction,
            correction
        );
        assert_eq!(
            std::fs::read(directory.path().join("settings.json")).unwrap(),
            bytes
        );
        let ended = controller
            .calibrate(CalibrationRequest::Cancel { token: tv.token })
            .await
            .unwrap();
        assert!(ended.ended.unwrap().contains("on the TV"));
        assert!(!controller.calibration.as_ref().unwrap().active());
        assert_eq!(controller.settings.view().settings, before.settings);
        assert!(
            controller
                .calibrate(CalibrationRequest::Poll {
                    token: phone.token,
                    compare: false,
                    playing: false
                })
                .await
                .unwrap()
                .ended
                .is_some()
        );
        controller
            .calibrate(CalibrationRequest::Begin)
            .await
            .unwrap();
        assert!(
            controller
                .calibrate(CalibrationRequest::Poll {
                    token: phone.token,
                    compare: false,
                    playing: false
                })
                .await
                .is_err()
        );
        controller.mode.shutdown().await.unwrap();
        server.abort();
    }

    fn stage_draft(
        controller: &mut Controller,
        tv: &CalibrationView,
        phone: &CalibrationView,
        correction: prysm_contract::ColorCorrection,
        step: CalibrationStep,
    ) {
        let now = std::time::Instant::now();
        let session = controller.calibration.as_mut().unwrap();
        session
            .transition(
                CalibrationRequest::Poll {
                    token: tv.token,
                    compare: false,
                    playing: true,
                },
                now,
            )
            .unwrap();
        session
            .transition(
                CalibrationRequest::Update {
                    token: phone.token,
                    sequence: session.view(phone.token, now, false).sequence + 1,
                    correction,
                    step,
                    shade: 0,
                    paused: false,
                    compare: false,
                },
                now,
            )
            .unwrap();
    }

    #[tokio::test]
    async fn calibration_save_failure_retains_draft_then_atomic_save_activates_and_replaces() {
        let (directory, mut controller, _, _receiver, server) = calibration_owner(true).await;
        let (tv, phone) = paired(&mut controller).await;
        controller
            .calibrate(CalibrationRequest::Poll {
                token: tv.token,
                playing: true,
                compare: false,
            })
            .await
            .unwrap();
        let _sender = tv_frame(&controller.webos).await;
        let started = controller.job.take().unwrap().await.unwrap();
        controller.finished(started).await;
        let normal = controller.settings.view();
        let approved = prysm_contract::ColorCorrection {
            tint: 25.try_into().unwrap(),
            ..prysm_contract::ColorCorrection::default()
        };
        stage_draft(
            &mut controller,
            &tv,
            &phone,
            approved,
            CalibrationStep::Review,
        );
        let path = directory.path().join("settings.json");
        std::fs::rename(&path, directory.path().join("backup.json")).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(matches!(
            controller
                .calibrate(CalibrationRequest::Save {
                    token: phone.token,
                    name: "PS5".into()
                })
                .await,
            Err(CommandError::Persistence(_))
        ));
        assert_eq!(controller.settings.view().settings, normal.settings);
        assert!(controller.calibration.as_ref().unwrap().active());
        std::fs::remove_dir(&path).unwrap();
        std::fs::rename(directory.path().join("backup.json"), &path).unwrap();
        controller
            .calibrate(CalibrationRequest::Save {
                token: phone.token,
                name: "PS5".into(),
            })
            .await
            .unwrap();
        let saved = controller.settings.view();
        assert_eq!(saved.settings.processing, normal.settings.processing);
        assert_eq!(saved.settings.calibration.correction(), approved);
        assert_eq!(
            SettingsStore::load(path).unwrap().view().settings,
            saved.settings
        );
        let id = saved.settings.calibration.active.unwrap();
        let mut inactive = saved.settings;
        inactive.calibration.active = None;
        controller
            .settings
            .update(prysm_contract::UpdateSettings {
                expected_revision: saved.revision,
                settings: inactive,
            })
            .await
            .unwrap();
        let (tv, phone) = paired(&mut controller).await;
        controller
            .calibrate(CalibrationRequest::Select {
                token: phone.token,
                profile: Some(id),
            })
            .await
            .unwrap();
        stage_draft(
            &mut controller,
            &tv,
            &phone,
            approved,
            CalibrationStep::Review,
        );
        controller
            .calibrate(CalibrationRequest::Save {
                token: phone.token,
                name: "PS5 updated".into(),
            })
            .await
            .unwrap();
        let saved = controller.settings.view();
        assert_eq!(saved.settings.calibration.profiles.len(), 1);
        assert_eq!(saved.settings.calibration.active, Some(id));
        controller.mode.shutdown().await.unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn calibration_timeouts_capture_interruption_and_stop_release_overrides_without_restart()
    {
        for cause in ["tv", "phone", "capture", "stop"] {
            let (_directory, mut controller, _, _receiver, server) = calibration_owner(false).await;
            let normal = controller.settings.view().settings;
            let (tv, phone) = paired(&mut controller).await;
            stage_draft(
                &mut controller,
                &tv,
                &phone,
                prysm_contract::ColorCorrection {
                    midtones: 60.try_into().unwrap(),
                    ..prysm_contract::ColorCorrection::default()
                },
                CalibrationStep::WhiteBalance,
            );
            if cause == "tv" || cause == "phone" {
                controller
                    .calibration
                    .as_mut()
                    .unwrap()
                    .transition(
                        CalibrationRequest::Poll {
                            token: if cause == "tv" { tv.token } else { phone.token },
                            compare: false,
                            playing: false,
                        },
                        std::time::Instant::now()
                            .checked_sub(Duration::from_secs(31))
                            .unwrap(),
                    )
                    .unwrap();
            }
            controller.apply_calibration().await.unwrap();
            if cause == "stop" {
                let (reply, result) = oneshot::channel();
                controller
                    .accept(Command::SetTarget(
                        prysm_contract::Target {
                            power: false,
                            source: Source::Prysm,
                        },
                        reply,
                    ))
                    .await;
                result.await.unwrap().unwrap();
                let finished = controller.job.take().unwrap().await.unwrap();
                controller.finished(finished).await;
            } else {
                controller.calibration_tick().await;
            }
            assert!(
                !controller.calibration.as_ref().unwrap().active(),
                "{cause}"
            );
            assert_eq!(
                controller.settings.view().settings.processing,
                normal.processing
            );
            assert_eq!(
                controller
                    .mode
                    .state(OperationStatus::Idle)
                    .unwrap()
                    .pipeline
                    .phase,
                prysm_contract::PipelinePhase::Idle
            );
            controller.mode.shutdown().await.unwrap();
            server.abort();
        }
    }
}
