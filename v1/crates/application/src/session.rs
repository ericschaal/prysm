//! Encapsulated lighting lifecycle; driven by the application task, never spawned.
//! Transition rules are in model.rs; device calls and acknowledgments in devices.rs.
mod devices;
mod model;
mod settings;

use devices::{Devices, Pending};
use futures::future::BoxFuture;
use model::{Action, Model};
use prysm_contract::{
    ControlError, Failure,
    application::{LightingTarget, SessionState, Settings},
    capture::{CaptureControl, CaptureState},
    identity::InstanceId,
    output::{IdleOutputMode, OutputControl},
    processing::ProcessingControl,
};
use prysm_core::EdgeColors;
use settings::SettingsApplication;
pub(super) use settings::validate_settings;

pub(super) enum Requested {
    Pending,
    Complete,
}
pub(super) enum Outcome {
    Changed,
    Ready,
    Failed(Failure),
    Stopped(Option<Failure>),
    Blocked(Failure),
}

/// A completion can settle both Off and an interrupted settings application.
pub(super) struct Progress {
    pub outcome: Outcome,
    pub settings: Option<Result<(), Failure>>,
}
impl From<Outcome> for Progress {
    fn from(outcome: Outcome) -> Self {
        Self {
            outcome,
            settings: None,
        }
    }
}

pub(super) struct Lifecycle<C: CaptureControl, P, O: OutputControl> {
    model: Model,
    devices: Devices<C, P, O>,
    pending: Option<Pending>,
    settings_application: Option<SettingsApplication<Settings<C::Settings, O::Settings>>>,
    opening: Option<BoxFuture<'static, Result<(), ControlError>>>,
}
impl<C: CaptureControl, P: ProcessingControl, O: OutputControl> Lifecycle<C, P, O>
where
    C::Settings: PartialEq,
    O::Settings: PartialEq,
{
    pub fn new(
        instance: InstanceId,
        controls: (C, P, O),
        settings: Settings<C::Settings, O::Settings>,
        waiting_colors: EdgeColors,
    ) -> Self {
        Self {
            model: Model::new(instance),
            devices: Devices::new(controls, settings, waiting_colors),
            pending: None,
            opening: None,
            settings_application: None,
        }
    }
    pub fn settings(&self) -> &Settings<C::Settings, O::Settings> {
        self.devices.settings()
    }
    pub fn status(&self) -> SessionState {
        self.model.status()
    }
    pub fn is_idle(&self) -> bool {
        self.model.is_idle()
    }
    pub fn request(&mut self, target: LightingTarget) -> Result<Requested, ControlError> {
        self.model.check_request(target)?;
        self.devices.validate(target)?;
        let result = self.model.request(target, self.settings().output.is_some());
        self.interrupt_open();
        Ok(result)
    }
    pub fn stop(&mut self) -> Progress {
        self.model.stop(IdleOutputMode::Off, true);
        self.interrupt_pending_calls(Outcome::Changed.into())
    }
    pub fn fail(&mut self, failure: Failure) -> Progress {
        self.model.fail(failure.clone());
        self.interrupt_pending_calls(Outcome::Failed(failure).into())
    }
    pub fn capture_changed(&mut self, state: &CaptureState) -> Progress {
        if self.model.capture_changed(state) {
            let CaptureState::Failed { failure, .. } = state else {
                unreachable!()
            };
            self.interrupt_pending_calls(Outcome::Failed(failure.clone()).into())
        } else {
            Outcome::Changed.into()
        }
    }
    fn interrupt_pending_calls(&mut self, mut progress: Progress) -> Progress {
        self.interrupt_open();
        match &mut self.settings_application {
            Some(SettingsApplication::Processing { call, candidate }) if call.started() => {
                *candidate = None;
            }
            Some(SettingsApplication::Processing { .. }) => {
                self.settings_application = None;
                progress.settings = Some(Ok(()));
            }
            _ => self.settings_application = None,
        }
        progress
    }
    fn interrupt_open(&mut self) {
        if self.model.is_stopping()
            && self
                .pending
                .as_ref()
                .is_some_and(|call| matches!(call.action, Action::Open(_)))
        {
            self.opening = self.pending.take().expect("capture open").interrupt_open();
            if self.opening.is_none() {
                self.model.discard_unissued_open();
            }
        }
    }
    /// Cancel-safe: select may drop this waiter; retained device calls stay here.
    /// Each completion emits one domain outcome; the coordinator never sees actions.
    pub async fn progress(&mut self) -> Progress {
        if self.pending.is_none()
            && let Some(action) = self.model.next_action()
            && !(matches!(action, Action::Barrier(_))
                && (self.opening.is_some()
                    || matches!(
                        self.settings_application,
                        Some(SettingsApplication::Processing { .. })
                    )))
        {
            if action == Action::InstallSettings {
                let Some(SettingsApplication::Restart(settings, revision)) =
                    self.settings_application.take()
                else {
                    unreachable!("settings restart retains its candidate")
                };
                self.devices.install(settings, revision);
                return self.model.completed(action, Ok(()));
            }
            self.model.scheduled(action);
            self.pending = Some(self.devices.begin(action));
        }
        enum Completed {
            Device(Result<(), ControlError>),
            Open,
            Processing(Result<(), ControlError>),
        }
        let completed = tokio::select! {
            result = async { self.pending.as_mut().expect("guarded").wait().await }, if self.pending.is_some() => Completed::Device(result),
            _ = async { self.opening.as_mut().expect("guarded").as_mut().await }, if self.opening.is_some() => Completed::Open,
            result = async {
                match &mut self.settings_application {
                    Some(SettingsApplication::Processing { call, .. }) => call.wait().await,
                    _ => std::future::pending().await,
                }
            } => Completed::Processing(result),
            else => std::future::pending().await,
        };
        let progress = match completed {
            Completed::Device(result) => {
                let action = self.pending.take().expect("completed call").action;
                self.model.completed(action, result)
            }
            Completed::Open => {
                self.opening = None;
                Outcome::Changed.into()
            }
            Completed::Processing(result) => return self.processing_updated(result),
        };
        if matches!(progress.outcome, Outcome::Failed(_) | Outcome::Blocked(_)) {
            return self.interrupt_pending_calls(progress);
        }
        if progress.settings.is_some() {
            self.settings_application = None;
        }
        progress
    }
}
